//! A routine's « when » in plain words, ready to show as it is (Alex 25/11, the screen « Préparer Santé et sport »:
//! « Ça veut dire quoi ces chiffres » about « 0 12 * * 1-5 »): never a cron expression on a screen. The apps show
//! these words as they come:
//!
//!     a model's walk, preview, share review   `display` (templates.rs, templates_catalogue.rs; a shared model's
//!                                             bundle carries them in `schedule.display`, the guichet's preview shows it)
//!     an agent's routines (row, page)         `schedule_shown` (routines.rs `view_of`)
//!     « Ce que tes agents font tout seuls »,  `when.shown` (autonomous.rs `listing`: routines, connectors' schedules,
//!     a plan's moments                        a plan's habit « Lun., mer. et ven. vers 7 h 30 » included)
//!     a routine's card                        `bip_routine.when.shown` (routines.rs)
//!
//! In the agent's language (as the bridge's other words: cards, frame, plans), the catalogue's in the one it is asked
//! for: fr, en, de, es, nl (any other: en). The words of the routines' screens (« Chaque jour à 8 h », « En semaine
//! vers 7 h 30 », « Le 3 de chaque mois à 9 h », « Toutes les 2 h »…; in French the hour « 12 h », « 12 h 30 » and one
//! day « Le dimanche », Alex 25/11), and for what they could not say: several days (« Le lundi, le mercredi et le
//! vendredi à 9 h »), the weekend, several times (« à 8 h et 18 h »), several days of the month, a cron's every N hours
//! or minutes; anything else: « Régulièrement » (never wrong for a repeating schedule). French keeps « 7 h 30 » on one
//! line (non-breaking spaces, as the apps' TaskFrame.clock).
//!
//! No Python counterpart.

use chrono::TimeZone;
use chrono_tz::Tz;
use serde_json::Value;

/// The languages said here; any other code: English.
pub const LANGS: [&str; 5] = ["fr", "en", "de", "es", "nl"];

/// At most this many times of day or days of the month in words; more: « Régulièrement ».
const MOST: usize = 4;

const NBSP: char = '\u{a0}';

/// « fr-FR », « Dutch », « nl »… → one of `LANGS`.
pub fn lang(code: &str) -> &'static str {
    let code = crate::souls::agent_language(Some(code));
    LANGS.iter().copied().find(|l| *l == code).unwrap_or("en")
}

/// The days a schedule runs on (1 = Monday … 7 = Sunday).
#[derive(Debug, Clone, PartialEq)]
pub enum Days {
    All,
    Weekdays,
    Weekend,
    /// Sorted, one to six days that are none of the above.
    Some(Vec<u32>),
}

/// What the words say.
#[derive(Debug, Clone, PartialEq)]
pub enum Plain {
    /// Those days, at those times of day (sorted).
    At {
        days: Days,
        times: Vec<(u32, u32)>,
    },
    /// Those days of the month (sorted), of those months (1 = January; empty: every month), at those times.
    Month {
        days: Vec<u32>,
        months: Vec<u32>,
        times: Vec<(u32, u32)>,
    },
    /// Every N hours (1: every hour).
    EveryHours(u32),
    EveryMinutes(u32),
    /// Once, at that instant when known.
    Once(Option<f64>),
    /// Any other repeating schedule.
    Regularly,
}

// -- reading --------------------------------------------------------------------------------------------------------

const WEEK_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTH_NAMES: [&str; 13] =
    ["", "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// A cron field's numbers: `5`, `1,3,5`, `1-5`, `MON,WED`, `mon-fri` (the `names`' positions), each within `lo..=hi`;
/// None for anything else (`*`, steps, out of range).
fn numbers(field: &str, lo: u32, hi: u32, names: &[&str]) -> Option<Vec<u32>> {
    let one = |s: &str| -> Option<u32> {
        let s = s.trim().to_lowercase();
        let n = match s.parse::<u32>() {
            Ok(n) => n,
            Err(_) => names.iter().position(|d| !d.is_empty() && *d == s)? as u32,
        };
        (lo..=hi).contains(&n).then_some(n)
    };
    let mut out = Vec::new();
    for part in field.split(',') {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (one(a)?, one(b)?);
                if a > b {
                    return None;
                }
                out.extend(a..=b);
            }
            None => out.push(one(part)?),
        }
    }
    out.sort_unstable();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

/// `*/N` → N (2 or more, below `limit`).
fn step(field: &str, limit: u32) -> Option<u32> {
    field.strip_prefix("*/")?.parse::<u32>().ok().filter(|n| (2..limit).contains(n))
}

fn days_of(week: &[u32]) -> Days {
    let mut d: Vec<u32> = week.iter().map(|d| if *d == 0 { 7 } else { *d }).collect();
    d.sort_unstable();
    d.dedup();
    match d.as_slice() {
        [1, 2, 3, 4, 5, 6, 7] => Days::All,
        [1, 2, 3, 4, 5] => Days::Weekdays,
        [6, 7] => Days::Weekend,
        _ => Days::Some(d),
    }
}

/// A cron expression (minute hour day-of-month month day-of-week) in what the words can say.
pub fn cron(expr: &str) -> Plain {
    let f: Vec<&str> = expr.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = f.as_slice() else { return Plain::Regularly };
    let any = |s: &str| s == "*" || s == "?";
    // some months only (« 0 9 1 1,4,7,10 * »: each quarter): one day of those months, nothing else
    let months = if any(month) {
        Vec::new()
    } else {
        match numbers(month, 1, 12, &MONTH_NAMES) {
            Some(m) if m.len() == 12 => Vec::new(),
            Some(m) => m,
            None => return Plain::Regularly,
        }
    };
    if !months.is_empty() && !(any(dow) && numbers(dom, 1, 31, &[]).is_some_and(|d| d.len() == 1)) {
        return Plain::Regularly;
    }
    if any(dom) && any(dow) {
        // every N minutes, every hour, every N hours
        if any(hour) {
            if let Some(n) = step(minute, 60) {
                return Plain::EveryMinutes(n);
            }
            if numbers(minute, 0, 59, &[]).is_some_and(|m| m.len() == 1) {
                return Plain::EveryHours(1);
            }
            return Plain::Regularly;
        }
        if let (Some(n), Some(m)) = (step(hour, 24), numbers(minute, 0, 59, &[])) {
            if m.len() == 1 {
                return Plain::EveryHours(n);
            }
        }
    }
    let (Some(minutes), Some(hours)) = (numbers(minute, 0, 59, &[]), numbers(hour, 0, 23, &[])) else {
        return Plain::Regularly;
    };
    let times: Vec<(u32, u32)> = hours.iter().flat_map(|h| minutes.iter().map(move |m| (*h, *m))).collect();
    if times.len() > MOST {
        return Plain::Regularly;
    }
    match (any(dom), any(dow)) {
        (true, true) => Plain::At { days: Days::All, times },
        (true, false) => match numbers(dow, 0, 7, &WEEK_NAMES) {
            Some(week) => Plain::At { days: days_of(&week), times },
            None => Plain::Regularly,
        },
        (false, true) => match numbers(dom, 1, 31, &[]) {
            Some(days) if days.len() <= MOST => Plain::Month { days, months, times },
            _ => Plain::Regularly,
        },
        // a day of the month OR a day of the week: rare, said plainly
        (false, false) => Plain::Regularly,
    }
}

/// autonomous.rs' `when` ({kind, hour, minute, weekday, day, hours, minutes, at, raw}) in what the words can say; None
/// for what is no schedule (`on_change`, `now`, `mail`, `link`).
pub fn of_when(when: &Value) -> Option<Plain> {
    let int = |k: &str| when.get(k).and_then(Value::as_u64).map(|n| n as u32);
    let time = || vec![(int("hour").unwrap_or(0).min(23), int("minute").unwrap_or(0).min(59))];
    let plain = match when.get("kind").and_then(Value::as_str).unwrap_or("") {
        "daily" => Plain::At { days: Days::All, times: time() },
        "weekdays" => Plain::At { days: Days::Weekdays, times: time() },
        "weekly" => Plain::At { days: days_of(&[int("weekday").unwrap_or(1).clamp(1, 7)]), times: time() },
        "monthly" => {
            Plain::Month { days: vec![int("day").unwrap_or(1).clamp(1, 31)], months: Vec::new(), times: time() }
        }
        "every_hours" => Plain::EveryHours(int("hours").unwrap_or(1).max(1)),
        "every_minutes" => match int("minutes").unwrap_or(0) {
            m if m >= 60 && m.is_multiple_of(60) => Plain::EveryHours(m / 60),
            0 | 1 => Plain::Regularly,
            m => Plain::EveryMinutes(m),
        },
        "once" => Plain::Once(when.get("at").and_then(Value::as_f64)),
        "custom" => when.get("raw").and_then(Value::as_str).map(cron).unwrap_or(Plain::Regularly),
        _ => return None,
    };
    Some(plain)
}

/// Hermes' `schedule` ({kind: cron, expr} / {kind: interval, minutes} / {kind: once, run_at: rfc 3339 or Unix
/// seconds}) in what the words can say.
pub fn of_schedule(schedule: &Value) -> Plain {
    let minutes = schedule.get("minutes").and_then(Value::as_u64);
    match schedule.get("kind").and_then(Value::as_str).unwrap_or("") {
        "cron" => cron(schedule.get("expr").and_then(Value::as_str).unwrap_or("")),
        "interval" => match minutes.unwrap_or(0) as u32 {
            m if m >= 60 && m.is_multiple_of(60) => Plain::EveryHours(m / 60),
            m if m >= 2 => Plain::EveryMinutes(m),
            _ => Plain::Regularly,
        },
        "once" => Plain::Once(schedule.get("run_at").or_else(|| schedule.get("at")).and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.timestamp() as f64)
            })
        })),
        _ => Plain::Regularly,
    }
}

// -- saying ---------------------------------------------------------------------------------------------------------

fn day_name(lang: &str, day: u32) -> &'static str {
    const FR: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
    const EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    const DE: [&str; 7] = ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"];
    const ES: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
    const NL: [&str; 7] = ["maandag", "dinsdag", "woensdag", "donderdag", "vrijdag", "zaterdag", "zondag"];
    let names = match lang {
        "fr" => FR,
        "de" => DE,
        "es" => ES,
        "nl" => NL,
        _ => EN,
    };
    names[(day.clamp(1, 7) - 1) as usize]
}

fn and(lang: &str) -> &'static str {
    match lang {
        "fr" => "et",
        "de" => "und",
        "es" => "y",
        "nl" => "en",
        _ => "and",
    }
}

/// « a, b et c ».
fn joined(lang: &str, items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} {} {last}", head.join(", "), and(lang)),
    }
}

/// « 8 h », « 12 h 30 », « 8:00 AM », « 8:00 ».
fn exact_clock(lang: &str, (h, m): (u32, u32)) -> String {
    match lang {
        // « 12 h », « 12 h 30 », « 8 h » (Alex 25/11), on one line
        "fr" => around_clock(lang, (h, m)),
        "en" => {
            format!("{}:{m:02} {}", if h.is_multiple_of(12) { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
        }
        _ => format!("{h}:{m:02}"),
    }
}

/// « 8 h », « 7 h 30 », « 8 AM », « 7:30 », « 8 »: a time « vers » it (the words of task_frame.rs `clock`).
fn around_clock(lang: &str, (h, m): (u32, u32)) -> String {
    match (lang, m) {
        ("fr", 0) => format!("{h}{NBSP}h"),
        ("fr", _) => format!("{h}{NBSP}h{NBSP}{m:02}"),
        ("en", _) => {
            let (h12, half) = (if h.is_multiple_of(12) { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" });
            if m == 0 {
                format!("{h12} {half}")
            } else {
                format!("{h12}:{m:02} {half}")
            }
        }
        (_, 0) => format!("{h}"),
        _ => format!("{h}:{m:02}"),
    }
}

/// « à 8 h », « à 8 h et 18 h », « vers 7 h 30 », « um 8:00 Uhr », « gegen 8 und 18 Uhr », « a la 1:00 »…
fn times_words(lang: &str, times: &[(u32, u32)], approx: bool) -> String {
    let clocks: Vec<String> =
        times.iter().map(|t| if approx { around_clock(lang, *t) } else { exact_clock(lang, *t) }).collect();
    let list = joined(lang, &clocks);
    match (lang, approx) {
        ("fr", false) => format!("à {list}"),
        ("fr", true) => format!("vers {list}"),
        ("de", false) => format!("um {list} Uhr"),
        ("de", true) => format!("gegen {list} Uhr"),
        ("es", false) if times.len() == 1 && times[0].0 == 1 => format!("a la {list}"),
        ("es", false) => format!("a las {list}"),
        ("es", true) => format!("hacia las {list}"),
        ("nl", false) => format!("om {list}"),
        ("nl", true) => format!("rond {list} uur"),
        (_, false) => format!("at {list}"),
        (_, true) => format!("around {list}"),
    }
}

fn days_words(lang: &str, days: &Days) -> String {
    match (lang, days) {
        ("fr", Days::All) => "Chaque jour".into(),
        ("fr", Days::Weekdays) => "En semaine".into(),
        ("fr", Days::Weekend) => "Le week-end".into(),
        ("de", Days::All) => "Jeden Tag".into(),
        ("de", Days::Weekdays) => "Werktags".into(),
        ("de", Days::Weekend) => "Am Wochenende".into(),
        ("es", Days::All) => "Cada día".into(),
        ("es", Days::Weekdays) => "Entre semana".into(),
        ("es", Days::Weekend) => "Los fines de semana".into(),
        ("nl", Days::All) => "Elke dag".into(),
        ("nl", Days::Weekdays) => "Op werkdagen".into(),
        ("nl", Days::Weekend) => "In het weekend".into(),
        (_, Days::All) => "Every day".into(),
        (_, Days::Weekdays) => "Weekdays".into(),
        (_, Days::Weekend) => "Weekends".into(),
        // « Le dimanche », « Le lundi, le mercredi et le vendredi » (Alex 25/11)
        ("fr", Days::Some(list)) => {
            let names: Vec<String> = list.iter().map(|d| format!("le {}", day_name(lang, *d))).collect();
            in_words_first_capital(&joined(lang, &names))
        }
        (_, Days::Some(list)) => {
            let names: Vec<String> = list.iter().map(|d| day_name(lang, *d).to_string()).collect();
            let each = match lang {
                "fr" => "Chaque",
                "de" => "Jeden",
                "es" => "Cada",
                "nl" => "Elke",
                _ => "Every",
            };
            format!("{each} {}", joined(lang, &names))
        }
    }
}

fn month_name(lang: &str, month: u32) -> &'static str {
    const FR: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const DE: [&str; 12] = [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ];
    const ES: [&str; 12] = [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ];
    const NL: [&str; 12] = [
        "januari",
        "februari",
        "maart",
        "april",
        "mei",
        "juni",
        "juli",
        "augustus",
        "september",
        "oktober",
        "november",
        "december",
    ];
    let names = match lang {
        "fr" => FR,
        "de" => DE,
        "es" => ES,
        "nl" => NL,
        _ => EN,
    };
    names[(month.clamp(1, 12) - 1) as usize]
}

/// « Le 3 de chaque mois », « Le 1er et le 15 de chaque mois », « On day 3 of each month », « Am 1. und 15. jedes
/// Monats », « Los días 1 y 15 de cada mes », « Op de 1e en 15e van elke maand »; some months only (one day): « Le 1er
/// janvier, avril, juillet et octobre », « On day 1 of January, April… », « Am 1. Januar, April… », « El día 1 de
/// enero, abril… », « Op 1 januari, april… ».
fn month_words(lang: &str, days: &[u32], months: &[u32]) -> String {
    if let ([day], false) = (days, months.is_empty()) {
        let names: Vec<String> = months.iter().map(|m| month_name(lang, *m).to_string()).collect();
        let names = joined(lang, &names);
        return match lang {
            "fr" => format!("Le {} {names}", if *day == 1 { "1er".to_string() } else { day.to_string() }),
            "de" => format!("Am {day}. {names}"),
            "es" => format!("El día {day} de {names}"),
            "nl" => format!("Op {day} {names}"),
            _ => format!("On day {day} of {names}"),
        };
    }
    let several = days.len() > 1;
    match lang {
        "fr" => {
            let each: Vec<String> = days
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let n = if *d == 1 { "1er".to_string() } else { d.to_string() };
                    format!("{} {n}", if i == 0 { "Le" } else { "le" })
                })
                .collect();
            format!("{} de chaque mois", joined(lang, &each))
        }
        "de" => {
            let each: Vec<String> = days.iter().map(|d| format!("{d}.")).collect();
            format!("Am {} jedes Monats", joined(lang, &each))
        }
        "es" => {
            let each: Vec<String> = days.iter().map(u32::to_string).collect();
            format!("{} {} de cada mes", if several { "Los días" } else { "El día" }, joined(lang, &each))
        }
        "nl" => {
            let each: Vec<String> = days.iter().map(|d| format!("{d}e")).collect();
            format!("Op de {} van elke maand", joined(lang, &each))
        }
        _ => {
            let each: Vec<String> = days.iter().map(u32::to_string).collect();
            format!("On {} {} of each month", if several { "days" } else { "day" }, joined(lang, &each))
        }
    }
}

/// « 30/11 », « Nov 30 », « 30.11. », « 30/11 », « 30-11 » (with its year when not this year).
fn day_of(lang: &str, at: f64, zone: Tz) -> String {
    match lang {
        "nl" => crate::task_frame::day_words("fr", at, zone).replace('/', "-"),
        _ => crate::task_frame::day_words(lang, at, zone),
    }
}

/// `plain` in `lang` (one of `LANGS`; another code: `lang` reads it), capitalised, « vers » its times when `approx`;
/// `zone`: a once's date.
pub fn say(lang: &str, plain: &Plain, approx: bool, zone: Tz) -> String {
    let lang = self::lang(lang);
    match plain {
        Plain::At { days, times } => format!("{} {}", days_words(lang, days), times_words(lang, times, approx)),
        Plain::Month { days, months, times } => {
            format!("{} {}", month_words(lang, days, months), times_words(lang, times, approx))
        }
        Plain::EveryHours(n) if *n <= 1 => match lang {
            "fr" => "Toutes les heures",
            "de" => "Jede Stunde",
            "es" => "Cada hora",
            "nl" => "Elk uur",
            _ => "Every hour",
        }
        .into(),
        Plain::EveryHours(n) => match lang {
            "fr" => format!("Toutes les {n}{NBSP}h"),
            "de" => format!("Alle {n} Stunden"),
            "es" => format!("Cada {n} horas"),
            "nl" => format!("Elke {n} uur"),
            _ => format!("Every {n} hours"),
        },
        Plain::EveryMinutes(n) => match lang {
            "fr" => format!("Toutes les {n} minutes"),
            "de" => format!("Alle {n} Minuten"),
            "es" => format!("Cada {n} minutos"),
            "nl" => format!("Elke {n} minuten"),
            _ => format!("Every {n} minutes"),
        },
        Plain::Once(None) => match lang {
            "fr" => "Une seule fois",
            "de" => "Einmal",
            "es" => "Una sola vez",
            "nl" => "Eén keer",
            _ => "Once",
        }
        .into(),
        Plain::Once(Some(at)) => {
            let day = day_of(lang, *at, zone);
            let time = zone
                .timestamp_opt(*at as i64, 0)
                .single()
                .map(|d| {
                    use chrono::Timelike;
                    times_words(lang, &[(d.hour(), d.minute())], false)
                })
                .unwrap_or_default();
            match lang {
                "fr" => format!("Une fois, le {day} {time}"),
                "de" => format!("Einmal, am {day} {time}"),
                "es" => format!("Una vez, el {day} {time}"),
                "nl" => format!("Eén keer, op {day} {time}"),
                _ => format!("Once, on {day} {time}"),
            }
        }
        Plain::Regularly => match lang {
            "fr" => "Régulièrement",
            "de" => "Regelmäßig",
            "es" => "Con regularidad",
            "nl" => "Regelmatig",
            _ => "Regularly",
        }
        .into(),
    }
}

/// A `when` in words (its `approx`: « vers »); None for what is no schedule.
pub fn when_words(lang: &str, when: &Value, zone: Tz) -> Option<String> {
    let approx = when.get("approx").and_then(Value::as_bool).unwrap_or(false);
    of_when(when).map(|plain| say(lang, &plain, approx, zone))
}

/// A Hermes `schedule` in words.
pub fn schedule_words(lang: &str, schedule: &Value, approx: bool, zone: Tz) -> String {
    say(lang, &of_schedule(schedule), approx, zone)
}

/// Their first letter capital (« le lundi… » → « Le lundi… »).
fn in_words_first_capital(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The words in a sentence: their first letter small (« …a créé la routine « X » : chaque lundi… »).
pub fn in_sentence(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Words that look like a cron expression (five fields of digits, `*`, `,`, `-`, `/`, day names): never on a screen.
pub fn looks_like_cron(text: &str) -> bool {
    let f: Vec<&str> = text.split_whitespace().collect();
    f.len() == 5
        && f.iter().all(|p| p.chars().all(|c| c.is_ascii_digit() || "*,-/?".contains(c) || c.is_ascii_alphabetic()))
        && f.iter().take(2).all(|p| p.chars().all(|c| c.is_ascii_digit() || "*,-/".contains(c)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PARIS: Tz = chrono_tz::Europe::Paris;

    fn fr(expr: &str) -> String {
        say("fr", &cron(expr), false, PARIS).replace(NBSP, " ")
    }

    #[test]
    fn the_models_routines_in_plain_french() {
        // the screen « Préparer Santé et sport » (sante.json)
        assert_eq!(fr("0 12 * * 1-5"), "En semaine à 12 h");
        assert_eq!(fr("0 19 * * 0"), "Le dimanche à 19 h");
        for item in crate::templates_catalogue::items() {
            for s in item["schedules"].as_array().unwrap() {
                for l in LANGS {
                    let words = schedule_words(l, s, false, PARIS);
                    assert!(!looks_like_cron(&words) && !words.chars().any(|c| c == '*'), "{l}: {words}");
                    assert_ne!(words, say(l, &Plain::Regularly, false, PARIS), "{l}: {s}");
                }
            }
        }
    }

    #[test]
    fn the_common_forms() {
        assert_eq!(fr("0 8 * * *"), "Chaque jour à 8 h");
        // « 12 h 30 » on one line, no zero before the hour (Alex 25/11)
        assert_eq!(say("fr", &cron("30 12 * * *"), false, PARIS), "Chaque jour à 12\u{a0}h\u{a0}30");
        assert_eq!(fr("5 8 * * *"), "Chaque jour à 8 h 05");
        assert_eq!(fr("30 7 * * MON-FRI"), "En semaine à 7 h 30");
        assert_eq!(fr("0 8 * * 1"), "Le lundi à 8 h");
        assert_eq!(fr("0 8 * * 7"), "Le dimanche à 8 h");
        assert_eq!(fr("0 9 * * 1,3,5"), "Le lundi, le mercredi et le vendredi à 9 h");
        assert_eq!(fr("0 9 * * mon,wed"), "Le lundi et le mercredi à 9 h");
        assert_eq!(fr("0 9 * * 1-3"), "Le lundi, le mardi et le mercredi à 9 h");
        assert_eq!(fr("0 10 * * 6,0"), "Le week-end à 10 h");
        assert_eq!(fr("0 10 * * 0-6"), "Chaque jour à 10 h");
        assert_eq!(fr("0 8,18 * * *"), "Chaque jour à 8 h et 18 h");
        assert_eq!(fr("0 9 3 * *"), "Le 3 de chaque mois à 9 h");
        assert_eq!(fr("0 9 1,15 * *"), "Le 1er et le 15 de chaque mois à 9 h");
        assert_eq!(fr("0 9 1 1,4,7,10 *"), "Le 1er janvier, avril, juillet et octobre à 9 h");
        assert_eq!(fr("0 9 1 * *"), "Le 1er de chaque mois à 9 h");
        assert_eq!(fr("0 9 1 1-12 *"), "Le 1er de chaque mois à 9 h");
        assert_eq!(fr("0 */2 * * *"), "Toutes les 2 h");
        assert_eq!(fr("15 * * * *"), "Toutes les heures");
        assert_eq!(fr("*/15 * * * *"), "Toutes les 15 minutes");
    }

    #[test]
    fn the_rest_reads_plainly_never_as_numbers() {
        for odd in [
            "0 8-18/2 * * *",
            "* * * * *",
            "0 9 1,15 6 *",
            "0 9 * 6 1",
            "0 9 1 * 1",
            "0 6,8,10,12,14 * * *",
            "nonsense",
            "",
            "*/1 * * * *",
        ] {
            assert_eq!(fr(odd), "Régulièrement", "{odd}");
        }
        assert_eq!(say("nl", &cron("0 8-18/2 * * *"), false, PARIS), "Regelmatig");
        assert!(
            looks_like_cron("0 12 * * 1-5") && looks_like_cron("30 7 * * MON-FRI") && looks_like_cron("*/5 * * * *")
        );
        assert!(
            !looks_like_cron("En semaine à 12 h") && !looks_like_cron("Le lundi, le mercredi et le vendredi à 9 h")
        );
    }

    #[test]
    fn five_languages() {
        let s = |l: &str, e: &str| say(l, &cron(e), false, PARIS);
        let a = |l: &str, e: &str| say(l, &cron(e), true, PARIS);
        assert_eq!(s("en", "0 12 * * 1-5"), "Weekdays at 12:00 PM");
        assert_eq!(s("en", "0 19 * * 0"), "Every Sunday at 7:00 PM");
        assert_eq!(s("en", "0 9 * * 1,3,5"), "Every Monday, Wednesday and Friday at 9:00 AM");
        assert_eq!(s("en", "0 9 1,15 * *"), "On days 1 and 15 of each month at 9:00 AM");
        assert_eq!(s("en", "0 9 1 1,4,7,10 *"), "On day 1 of January, April, July and October at 9:00 AM");
        assert_eq!(s("de", "0 9 1 jan,jul *"), "Am 1. Januar und Juli um 9:00 Uhr");
        assert_eq!(s("es", "0 9 1 1,4,7,10 *"), "El día 1 de enero, abril, julio y octubre a las 9:00");
        assert_eq!(s("nl", "0 9 1 1,4,7,10 *"), "Op 1 januari, april, juli en oktober om 9:00");
        assert_eq!(s("de", "0 12 * * 1-5"), "Werktags um 12:00 Uhr");
        assert_eq!(s("de", "0 19 * * 0"), "Jeden Sonntag um 19:00 Uhr");
        assert_eq!(s("de", "0 8,18 * * *"), "Jeden Tag um 8:00 und 18:00 Uhr");
        assert_eq!(s("de", "0 9 3 * *"), "Am 3. jedes Monats um 9:00 Uhr");
        assert_eq!(s("es", "0 12 * * 1-5"), "Entre semana a las 12:00");
        assert_eq!(s("es", "0 1 * * *"), "Cada día a la 1:00");
        assert_eq!(s("es", "0 9 * * 6,0"), "Los fines de semana a las 9:00");
        assert_eq!(s("es", "0 9 1,15 * *"), "Los días 1 y 15 de cada mes a las 9:00");
        assert_eq!(s("nl", "0 12 * * 1-5"), "Op werkdagen om 12:00");
        assert_eq!(s("nl", "0 19 * * 0"), "Elke zondag om 19:00");
        assert_eq!(s("nl", "0 9 3 * *"), "Op de 3e van elke maand om 9:00");
        assert_eq!(s("nl", "0 */3 * * *"), "Elke 3 uur");
        assert_eq!(s("Dutch", "0 9 * * 2,4"), "Elke dinsdag en donderdag om 9:00");
        assert_eq!(s("pt-BR", "0 8 * * *"), "Every day at 8:00 AM");
        // « vers »: task_frame.rs' clock, several times too
        assert_eq!(a("fr", "30 7 * * *"), "Chaque jour vers 7\u{a0}h\u{a0}30");
        assert_eq!(a("fr", "37 7 * * 1,3,5").replace(NBSP, " "), "Le lundi, le mercredi et le vendredi vers 7 h 37");
        assert_eq!(a("en", "0 8,18 * * *"), "Every day around 8 AM and 6 PM");
        assert_eq!(a("de", "30 7 * * 1-5"), "Werktags gegen 7:30 Uhr");
        assert_eq!(a("es", "0 8 * * *"), "Cada día hacia las 8");
        assert_eq!(a("nl", "0 8 * * *"), "Elke dag rond 8 uur");
        for (l, h, m) in [("fr", 8, 0), ("fr", 7, 30), ("en", 0, 5), ("de", 7, 30), ("es", 19, 0)] {
            let line = say(l, &Plain::At { days: Days::All, times: vec![(h, m)] }, true, PARIS).replace(NBSP, " ");
            assert!(line.ends_with(&crate::task_frame::clock(l, h, m)), "{line}");
        }
    }

    #[test]
    fn whens_and_schedules() {
        let w = |v: Value| when_words("fr", &v, PARIS).map(|t| t.replace(NBSP, " "));
        assert_eq!(w(json!({"kind": "daily", "hour": 8, "minute": 0})).as_deref(), Some("Chaque jour à 8 h"));
        assert_eq!(
            w(json!({"kind": "weekly", "weekday": 3, "hour": 7, "minute": 5, "approx": true})).as_deref(),
            Some("Le mercredi vers 7 h 05")
        );
        assert_eq!(
            w(json!({"kind": "monthly", "day": 1, "hour": 9, "minute": 0})).as_deref(),
            Some("Le 1er de chaque mois à 9 h")
        );
        assert_eq!(w(json!({"kind": "every_hours", "hours": 1})).as_deref(), Some("Toutes les heures"));
        assert_eq!(w(json!({"kind": "every_hours", "hours": 3})).as_deref(), Some("Toutes les 3 h"));
        assert_eq!(w(json!({"kind": "every_minutes", "minutes": 30})).as_deref(), Some("Toutes les 30 minutes"));
        assert_eq!(w(json!({"kind": "once"})).as_deref(), Some("Une seule fois"));
        assert_eq!(
            w(json!({"kind": "custom", "raw": "30 7 * * 1,3,5", "approx": true})).as_deref(),
            Some("Le lundi, le mercredi et le vendredi vers 7 h 30")
        );
        assert_eq!(w(json!({"kind": "custom", "raw": "0 8-18/2 * * *"})).as_deref(), Some("Régulièrement"));
        assert_eq!(w(json!({"kind": "on_change"})), None);
        assert_eq!(w(json!({"kind": "mail", "from": "banque.fr"})), None);
        // a date: the routines' clock
        let at = PARIS.with_ymd_and_hms(2031, 11, 30, 18, 0, 0).unwrap().timestamp() as f64;
        assert_eq!(w(json!({"kind": "once", "at": at})).as_deref(), Some("Une fois, le 30/11/2031 à 18 h"));
        assert_eq!(say("en", &Plain::Once(Some(at)), false, PARIS), "Once, on Nov 30, 2031 at 6:00 PM");
        assert_eq!(say("de", &Plain::Once(Some(at)), false, PARIS), "Einmal, am 30.11.2031 um 18:00 Uhr");
        assert_eq!(say("nl", &Plain::Once(Some(at)), false, PARIS), "Eén keer, op 30-11-2031 om 18:00");
        // Hermes' schedules
        let sch = |v: Value| schedule_words("fr", &v, false, PARIS).replace(NBSP, " ");
        assert_eq!(sch(json!({"kind": "cron", "expr": "0 7 * * *"})), "Chaque jour à 7 h");
        assert_eq!(sch(json!({"kind": "interval", "minutes": 120})), "Toutes les 2 h");
        assert_eq!(sch(json!({"kind": "interval", "minutes": 45})), "Toutes les 45 minutes");
        assert_eq!(
            sch(json!({"kind": "once", "run_at": "2031-11-30T18:00:00+01:00"})),
            "Une fois, le 30/11/2031 à 18 h"
        );
        assert_eq!(in_sentence("Le lundi à 8 h"), "le lundi à 8 h");
    }
}
