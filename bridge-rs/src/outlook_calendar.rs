//! Outlook's calendars (outlook.rs) through the calendar tools the agents already have (calendar.rs), as Google
//! Calendar's (google_calendar.rs): `calendar_list` lists them beside the phone's, the mailboxes' and Google's
//! (« Calendrier (Outlook) »), `calendar_add` writes to the one it names (the default one when it names Outlook
//! itself, or when the agent has no other calendar), `calendar_update` and `calendar_delete` act on its events. Ids:
//! `outlook:<n>` for a calendar, `outlook:<n>:<m>` for an event (outlook.rs' short numbers for Graph's ids).
//!
//! **Every write asks the person first**, on Google Calendar's card (person_ok.rs): « Oui », « Toujours autoriser »
//! (adds and changes in the default calendar; kept under its own key, `calendar_write:outlook`: a « Toujours » for
//! Google's never lets Outlook's through) or « Non »; a deletion is asked each time (`calendar_delete`), and so is a
//! write in another calendar. Never silently: the kind is critical, at every level. Refused before any card: an event
//! that repeats, and a **meeting with guests** (Graph would tell them of the change: the person does it in Outlook).
//! A change or a deletion carries the event's etag (`If-Match`): an event changed since its card is left as it is.
//!
//! Times: Graph's are read in UTC and shown in the place's time zone (routines.rs `zone`); a time the agent writes
//! with an offset, or a local one (in that zone), goes to Graph in UTC; an all-day event by its dates.
//!
//! Not a Python module.

use chrono::{Duration, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::calendar::Moment;
use crate::config::{AgentConfig, Config};
use crate::drives::{Api, Fault};
use crate::google_calendar::{folded, mixed, when_of, MIXED, READ_ONLY};
use crate::outlook::{self, AccountId, LABEL, PREFER_IMMUTABLE};
use crate::tools_knowledge::{self as tk, tool_text};
use crate::Dict;

pub const ID_PREFIX: &str = "outlook:";
/// Calendars listed at most, the default one first.
const MAX_CALENDARS: usize = 20;
const PAGE: &str = "250";
/// Events read at most from a calendar for one listing (its pages followed).
const EVENTS_MAX: usize = 1000;
/// The names that mean the default calendar.
const OUTLOOK_NAMES: [&str; 6] =
    ["outlook", "outlook calendar", "agenda outlook", "calendrier outlook", "outlook-kalender", "calendario de outlook"];

/// A calendar of the account.
#[derive(Debug, Clone, PartialEq)]
pub struct Calendar {
    /// Graph's id.
    pub key: String,
    /// Its short number (the agents' ids).
    pub n: u64,
    /// As the person reads it in Outlook, on one line, defused (a shared one's name is written by others).
    pub name: String,
    pub writable: bool,
    pub default: bool,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A calendar's id for the agents.
pub fn calendar_ref(n: u64) -> String {
    format!("{ID_PREFIX}{n}")
}

/// An event's.
pub fn event_ref(calendar: u64, event: u64) -> String {
    format!("{ID_PREFIX}{calendar}:{event}")
}

/// The calendar's and the event's short numbers an agent's id names.
pub fn split_ref(id: &str) -> Option<(u64, u64)> {
    let (c, e) = id.trim().strip_prefix(ID_PREFIX)?.split_once(':')?;
    Some((c.trim().parse().ok()?, e.trim().parse().ok()?))
}

/// Graph's calendar of `account` as kept here. None: no id.
pub fn calendar_of(config: &Config, account: AccountId, v: &Value) -> Option<Calendar> {
    calendar_with(v, outlook::short_id(config, account, &text(v, "id")))
}

fn calendar_with(v: &Value, n: u64) -> Option<Calendar> {
    let key = text(v, "id");
    if key.is_empty() {
        return None;
    }
    let name = crate::archives::defused(tk::head(&text(v, "name").split_whitespace().collect::<Vec<_>>().join(" "), 120));
    Some(Calendar {
        n,
        name: if name.is_empty() { LABEL.to_string() } else { name },
        writable: v.get("canEdit").and_then(Value::as_bool).unwrap_or(false),
        default: v.get("isDefaultCalendar").and_then(Value::as_bool).unwrap_or(false),
        key,
    })
}

/// Account `account`'s calendars, the default one first, `MAX_CALENDARS` at most.
pub async fn calendars(config: &Config, account: AccountId, api: &Api) -> Result<Vec<Calendar>, Fault> {
    let url = format!("{}/me/calendars", outlook::base(api));
    let query = [("$top", "50".to_string()), ("$select", "id,name,canEdit,isDefaultCalendar".to_string())];
    let (items, _) = outlook::pages(api, &url, &query, &[PREFER_IMMUTABLE], 100).await?;
    // their short numbers by one lot (review C3)
    let shorts = outlook::short_ids(config, account, &items.iter().map(|v| text(v, "id")).collect::<Vec<_>>());
    let mut found: Vec<Calendar> = items.iter().zip(shorts).filter_map(|(v, n)| calendar_with(v, n)).collect();
    // none marked (an older account): its first one is Outlook's own « Calendar »
    if !found.iter().any(|c| c.default) {
        if let Some(first) = found.first_mut() {
            first.default = true;
        }
    }
    found.sort_by_key(|c| !c.default);
    found.truncate(MAX_CALENDARS);
    Ok(found)
}

/// The place's time zone (Graph's times are read in UTC, shown in it).
fn zone(config: &Config) -> chrono_tz::Tz {
    crate::routines::zone(config)
}

/// Graph's `{dateTime, timeZone}` as calendar.rs writes times: an all-day event's date, else a date-time with its
/// offset in the place's zone.
pub fn moment_of(v: &Value, all_day: bool, zone: chrono_tz::Tz) -> String {
    let at = v.get("dateTime").and_then(Value::as_str).unwrap_or("");
    let naive = at.get(..19).and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok());
    let Some(naive) = naive else { return String::new() };
    if all_day {
        return naive.date().to_string();
    }
    let given = v.get("timeZone").and_then(Value::as_str).unwrap_or("UTC");
    let utc = match given.parse::<chrono_tz::Tz>() {
        Ok(tz) if given != "UTC" => tz.from_local_datetime(&naive).earliest().map(|t| t.naive_utc()).unwrap_or(naive),
        _ => naive,
    };
    chrono::Utc.from_utc_datetime(&utc).with_timezone(&zone).to_rfc3339()
}

/// A Graph event as calendar.rs lists them. None: cancelled, no start.
pub fn event_of(config: &Config, account: AccountId, v: &Value, calendar: u64, zone: chrono_tz::Tz) -> Option<Value> {
    event_with(v, calendar, outlook::short_id(config, account, &text(v, "id")), zone)
}

/// Graph's events of a page, their short numbers made by one lot (review C3).
pub fn events_of(config: &Config, account: AccountId, items: &[Value], calendar: u64, zone: chrono_tz::Tz) -> Vec<Value> {
    let shorts = outlook::short_ids(config, account, &items.iter().map(|v| text(v, "id")).collect::<Vec<_>>());
    items.iter().zip(shorts).filter_map(|(v, n)| event_with(v, calendar, n, zone)).collect()
}

fn event_with(v: &Value, calendar: u64, n: u64, zone: chrono_tz::Tz) -> Option<Value> {
    if v.get("isCancelled").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let id = text(v, "id");
    let all_day = v.get("isAllDay").and_then(Value::as_bool).unwrap_or(false);
    let start = moment_of(v.get("start")?, all_day, zone);
    let mut end = v.get("end").map(|e| moment_of(e, all_day, zone)).unwrap_or_default();
    if id.is_empty() || start.is_empty() {
        return None;
    }
    if all_day {
        // Graph's end is the day after the last one
        if let (Ok(last), Ok(first)) =
            (NaiveDate::parse_from_str(&end, "%Y-%m-%d"), NaiveDate::parse_from_str(&start, "%Y-%m-%d"))
        {
            end = (last - Duration::days(1)).max(first).to_string();
        }
    }
    let defused = |s: String, max: usize| crate::archives::defused(tk::head(&s, max));
    let kind = text(v, "type");
    Some(json!({
        "id": event_ref(calendar, n), "calendar": calendar_ref(calendar),
        "title": defused(text(v, "subject"), 300), "start": start, "end": end, "all_day": all_day,
        "location": defused(v.pointer("/location/displayName").and_then(Value::as_str).unwrap_or("").to_string(), 300),
        "notes": defused(text(v, "bodyPreview"), 2000),
        "recurring": kind == "occurrence" || kind == "exception" || kind == "seriesMaster"
            || v.get("seriesMasterId").and_then(Value::as_str).is_some_and(|s| !s.is_empty()),
    }))
}

/// What Outlook's calendars add to `calendar_list` for `agent`: each account's, and their events of the period, read
/// now (with several accounts, each calendar's source names its account).
pub async fn listed(config: &Config, agent: &str, remote: &mut crate::caldav::Remote, from: NaiveDate, to: NaiveDate) {
    for account in outlook::accounts_with(config, agent, "calendar") {
        listed_of(config, account.id, remote, from, to).await;
    }
}

async fn listed_of(config: &Config, account: AccountId, remote: &mut crate::caldav::Remote, from: NaiveDate, to: NaiveDate) {
    remote.any = true;
    let label = outlook::label(config, account);
    let failed = |remote: &mut crate::caldav::Remote, fault: &Fault| {
        remote.calendars.push(json!({"id": format!("{ID_PREFIX}0"), "title": LABEL, "source": label}));
        remote.failed.push(format!("{label}: {}", outlook::words(fault)));
    };
    let api = match outlook::signed_in(config, account).await {
        Ok(api) => api,
        Err(fault) => return failed(remote, &fault),
    };
    let found = match calendars(config, account, &api).await {
        Ok(found) => found,
        Err(fault) => return failed(remote, &fault),
    };
    let zone = zone(config);
    let stamp = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map(|t| t.and_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default();
    let query = [
        ("startDateTime", stamp(from - Duration::days(1))),
        ("endDateTime", stamp(to + Duration::days(2))),
        ("$top", PAGE.to_string()),
        ("$select", "id,subject,start,end,isAllDay,location,bodyPreview,type,seriesMasterId,isCancelled".to_string()),
        ("$orderby", "start/dateTime".to_string()),
    ];
    let base = outlook::base(&api);
    let urls: Vec<String> = found.iter().map(|c| format!("{base}/me/calendars/{}/calendarView", c.key)).collect();
    // every page of the period (review R10), `EVENTS_MAX` at most a calendar, said when some are left
    let pages = futures::future::join_all(
        urls.iter().map(|url| outlook::pages(&api, url, &query, &[PREFER_IMMUTABLE], EVENTS_MAX)),
    )
    .await;
    for (calendar, page) in found.iter().zip(pages) {
        remote.calendars.push(json!({"id": calendar_ref(calendar.n), "title": calendar.name, "source": label}));
        match page {
            Ok((items, more)) => {
                remote.events.extend(events_of(config, account, &items, calendar.n, zone));
                if more {
                    remote.failed.push(format!(
                        "{label}, one of its calendars: more than {EVENTS_MAX} events in the period, the first ones only"
                    ));
                }
            }
            Err(fault) => remote.failed.push(format!("{label}, one of its calendars: {}", outlook::words(&fault))),
        }
    }
}

// -- the agents' writes ------------------------------------------------------------------------------------------------

/// Whether a calendar write is for Outlook's calendars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    No,
    Outlook,
    /// An add naming a calendar the agent's others don't have: Outlook's if one of its calendars has that name.
    IfNamed,
}

/// Which calendar a write tool's call is for. `others`: the agent has another calendar (the phone's, a mailbox's,
/// Google's); `accounts`: its Outlook accounts' addresses; `elsewhere`: the other calendars' names (a bare name they
/// share stays theirs).
pub fn aim(name: &str, arguments: &Dict, others: bool, accounts: &[String], elsewhere: &[String]) -> Aim {
    match name {
        "calendar_update" | "calendar_delete" => {
            if tk::str_or_empty(arguments.get("id")).trim().starts_with(ID_PREFIX) {
                Aim::Outlook
            } else {
                Aim::No
            }
        }
        "calendar_add" => {
            let wanted = folded(&tk::str_or_empty(arguments.get("calendar")));
            if wanted.is_empty() {
                return if others { Aim::No } else { Aim::Outlook };
            }
            if OUTLOOK_NAMES.contains(&wanted.as_str())
                || wanted.starts_with(ID_PREFIX)
                || wanted.ends_with("(outlook)")
                || accounts.iter().any(|a| folded(a) == wanted || wanted.ends_with(&format!("({})", folded(a))))
            {
                Aim::Outlook
            } else if elsewhere.iter().any(|n| folded(n) == wanted) {
                Aim::No
            } else {
                Aim::IfNamed
            }
        }
        _ => Aim::No,
    }
}

/// The calendar of one account an add names: Outlook itself, its address or nothing → the default one; else the one
/// of that name (« (Outlook) » or « (its address) » after it or not), or its id.
pub fn pick<'a>(found: &'a [Calendar], wanted: &str, account: Option<&str>) -> Option<&'a Calendar> {
    let wanted = folded(wanted);
    let address = account.map(folded).unwrap_or_default();
    let mut base = wanted.strip_suffix("(outlook)").map(str::trim_end).unwrap_or(&wanted).to_string();
    if !address.is_empty() {
        for suffix in [format!("(outlook ({address}))"), format!("({address})")] {
            if let Some(b) = base.strip_suffix(&suffix) {
                base = b.trim_end().to_string();
            }
        }
        if let Some(b) = base.strip_prefix("outlook (").and_then(|b| b.strip_suffix(')')) {
            base = b.trim().to_string();
        }
    }
    if base.is_empty() || OUTLOOK_NAMES.contains(&base.as_str()) || (!address.is_empty() && address == base) {
        return found.iter().find(|c| c.default);
    }
    found.iter().find(|c| folded(&c.name) == base || calendar_ref(c.n) == wanted)
}

/// Whether `wanted` (an add's `calendar`) names account `address` itself (its address, « Outlook (address) », or a
/// calendar « … (address) »).
fn names_account(wanted: &str, address: &str) -> bool {
    let (wanted, address) = (folded(wanted), folded(address));
    !address.is_empty() && (wanted == address || wanted.ends_with(&format!("({address})")) || wanted.ends_with(&format!("({address}))")))
}

pub const NO_SUCH_CALENDAR: &str = "no Outlook calendar of that name: their names are in calendar_list \
(« name (Outlook) »).";
pub const REPEATING: &str = "a repeating event: the person changes or deletes it in Outlook (tell them)";
pub const MEETING: &str = "a meeting with guests: changing or deleting it here would write to them. The person does \
it in Outlook (tell them); nothing was asked nor changed";
pub const CHANGED: &str = "the event changed in Outlook since it was read: nothing was changed. Read it again with \
calendar_list and ask your person again if it still needs doing";

/// What the card shows and what is sent to Graph, once checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub tool: &'static str,
    /// Graph's calendar id, and its short number.
    pub calendar: String,
    pub calendar_n: u64,
    /// Its name, shown on the card when it is not the default one.
    pub calendar_name: Option<String>,
    /// Graph's event id (not for an add).
    pub event: Option<String>,
    pub etag: Option<String>,
    pub title: String,
    pub when: String,
    pub location: String,
    pub body: Option<Value>,
}

impl Planned {
    /// What a held « Oui » grants exactly.
    pub fn command(&self) -> String {
        let body = self.body.as_ref().map(|b| b.to_string()).unwrap_or_default();
        let target = match &self.event {
            Some(id) => format!("{}:{id}", calendar_ref(self.calendar_n)),
            None => calendar_ref(self.calendar_n),
        };
        format!("{} {target} {body}", self.tool).trim_end().to_string()
    }
}

/// Graph's `{dateTime, timeZone}` of a moment: UTC for a time (a local one read in the place's zone), midnight of the
/// day for an all-day event (`end`: the day after).
pub fn graph_time(moment: Moment, end: bool, zone: chrono_tz::Tz) -> Value {
    let utc = |naive: NaiveDateTime| json!({"dateTime": naive.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"});
    match moment {
        Moment::Day(day) => {
            let day = if end { day + Duration::days(1) } else { day };
            json!({"dateTime": format!("{day}T00:00:00"), "timeZone": "UTC"})
        }
        Moment::Zoned(at) => utc(at.naive_utc()),
        Moment::Local(at) => utc(zone.from_local_datetime(&at).earliest().map(|t| t.naive_utc()).unwrap_or(at)),
    }
}

/// An event of `calendar` (Graph's ids; a calendar shared with the person is reached by its own path).
fn event_url(api: &Api, calendar: &str, id: &str) -> String {
    format!("{}/me/calendars/{calendar}/events/{id}", outlook::base(api))
}

/// The event, read by its calendar's path, with the calendar it is really in (`$expand`): Exchange's event ids are
/// global, the path alone proves nothing (review C2).
async fn read_event(api: &Api, calendar: &str, id: &str) -> Result<Value, Fault> {
    let url = event_url(api, calendar, id);
    let select = "id,subject,start,end,isAllDay,location,bodyPreview,type,seriesMasterId,attendees,isOrganizer,isCancelled";
    let query = [("$select", select.to_string()), ("$expand", "calendar($select=id)".to_string())];
    outlook::fetch(api, &url, &query, &[PREFER_IMMUTABLE]).await
}

/// Whether the event read is in `calendar` (Graph's id of the card's calendar); not said: not proved, refused.
pub fn in_calendar(raw: &Value, calendar: &str) -> bool {
    raw.pointer("/calendar/id").and_then(Value::as_str).is_some_and(|id| id == calendar)
}

/// A meeting: guests listed, or someone else's invitation (its guests may be hidden, review R2).
pub fn is_meeting(raw: &Value) -> bool {
    raw.get("attendees").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
        || raw.get("isOrganizer").and_then(Value::as_bool) == Some(false)
}

fn fault_result(fault: &Fault) -> Dict {
    match fault {
        Fault::Changed => tool_text(CHANGED, true),
        other => outlook::fault_result(other, "I can't reach your Outlook calendar just now."),
    }
}

/// The call checked against Graph before any card. Err: the tool's answer (nothing asked, nothing sent).
pub async fn plan(config: &Config, account: AccountId, api: &Api, name: &str, arguments: &Dict, calendar: &Calendar)
    -> Result<Planned, Dict> {
    let zone = zone(config);
    let base = Planned {
        tool: "calendar_add",
        calendar: calendar.key.clone(),
        calendar_n: calendar.n,
        calendar_name: (!calendar.default).then(|| calendar.name.clone()),
        event: None,
        etag: None,
        title: String::new(),
        when: String::new(),
        location: String::new(),
        body: None,
    };
    match name {
        "calendar_add" => {
            let event = crate::calendar::new_event(arguments).map_err(|why| tool_text(&why, true))?;
            let start = Moment::parse(&text(&event, "start")).ok_or_else(|| tool_text("start: an ISO date-time", true))?;
            let end = Moment::parse(&text(&event, "end")).unwrap_or(start);
            if mixed(start, end) {
                return Err(tool_text(MIXED, true));
            }
            let all_day = matches!(start, Moment::Day(_));
            let mut body = json!({"subject": text(&event, "title"), "start": graph_time(start, false, zone),
                                  "end": graph_time(end, true, zone), "isAllDay": all_day});
            if !text(&event, "location").is_empty() {
                body["location"] = json!({"displayName": text(&event, "location")});
            }
            if !text(&event, "notes").is_empty() {
                body["body"] = json!({"contentType": "text", "content": text(&event, "notes")});
            }
            Ok(Planned {
                title: text(&event, "title"),
                when: when_of(&event),
                location: text(&event, "location"),
                body: Some(body),
                ..base
            })
        }
        "calendar_update" | "calendar_delete" => {
            let unknown = || tool_text("unknown event id (see calendar_list)", true);
            let (_, e) = split_ref(&tk::str_or_empty(arguments.get("id"))).ok_or_else(unknown)?;
            let id = outlook::long_id_of(config, account, e).ok_or_else(unknown)?;
            let raw = read_event(api, &calendar.key, &id).await.map_err(|f| fault_result(&f))?;
            // an event of another calendar under this one's number: no such event here, nothing asked (review C2)
            if !in_calendar(&raw, &calendar.key) {
                return Err(unknown());
            }
            let current =
                event_of(config, account, &raw, calendar.n, zone).ok_or_else(|| tool_text("no such event any more", true))?;
            if current.get("recurring").and_then(Value::as_bool).unwrap_or(false) {
                return Err(tool_text(REPEATING, true));
            }
            if is_meeting(&raw) {
                return Err(tool_text(MEETING, true));
            }
            let etag = Some(text(&raw, "@odata.etag")).filter(|e| !e.is_empty());
            if name == "calendar_delete" {
                return Ok(Planned {
                    tool: "calendar_delete",
                    event: Some(id),
                    etag,
                    title: text(&current, "title"),
                    when: when_of(&current),
                    location: text(&current, "location"),
                    ..base
                });
            }
            let mut merged = Dict::new();
            for key in ["title", "location", "notes", "start", "end"] {
                merged.insert(key.into(), json!(text(&current, key)));
            }
            let mut body = Dict::new();
            for (key, max) in [("title", 300), ("location", 300), ("notes", 2000)] {
                if arguments.contains_key(key) {
                    let value = tk::head(&tk::str_or_empty(arguments.get(key)), max).to_string();
                    merged.insert(key.into(), json!(value));
                    match key {
                        "title" => body.insert("subject".into(), json!(value)),
                        "location" => body.insert("location".into(), json!({"displayName": value})),
                        _ => body.insert("body".into(), json!({"contentType": "text", "content": value})),
                    };
                }
            }
            let given = |k: &str| Some(tk::str_or_empty(arguments.get(k)).trim().to_string()).filter(|v| !v.is_empty());
            let (old_start, old_end) = (Moment::parse(&text(&current, "start")), Moment::parse(&text(&current, "end")));
            let start = match given("start") {
                Some(v) => Some(Moment::parse(&v).ok_or_else(|| tool_text("start: an ISO date-time or a date", true))?),
                None => None,
            };
            let end = match given("end") {
                Some(v) => Some(Moment::parse(&v).ok_or_else(|| tool_text("end: an ISO date-time or a date", true))?),
                None => match (start, old_start, old_end) {
                    (Some(new), Some(s), Some(e)) => Some(new.plus((e.wall() - s.wall()).num_minutes())),
                    _ => None,
                },
            };
            if start.is_some() || end.is_some() {
                let (Some(s), Some(e)) = (start.or(old_start), end.or(old_end)) else {
                    return Err(tool_text("start: an ISO date-time or a date", true));
                };
                if mixed(s, e) {
                    return Err(tool_text(MIXED, true));
                }
                if e.wall() < s.wall() {
                    return Err(tool_text("the end must come after the start", true));
                }
                let all_day = matches!(s, Moment::Day(_));
                body.insert("start".into(), graph_time(s, false, zone));
                body.insert("end".into(), graph_time(e, true, zone));
                body.insert("isAllDay".into(), json!(all_day));
                merged.insert("start".into(), json!(s.write()));
                merged.insert("end".into(), json!(e.write()));
                merged.insert("all_day".into(), json!(all_day));
            } else {
                merged.insert("all_day".into(), current.get("all_day").cloned().unwrap_or(json!(false)));
            }
            if body.is_empty() {
                return Err(tool_text("nothing to change", true));
            }
            let shown = Value::Object(merged);
            Ok(Planned {
                tool: "calendar_update",
                event: Some(id),
                etag,
                title: text(&shown, "title"),
                when: when_of(&shown),
                location: text(&shown, "location"),
                body: Some(Value::Object(body)),
                ..base
            })
        }
        _ => Err(tool_text(&format!("unknown tool: {name}"), true)),
    }
}

/// What was planned, done at Graph (after the person's « Oui »).
pub async fn apply(config: &Config, account: AccountId, api: &Api, planned: &Planned) -> Dict {
    let label = outlook::label(config, account);
    let base = outlook::base(api);
    let (body, etag) = (planned.body.clone(), planned.etag.clone());
    use reqwest::Method;
    let done = match (planned.tool, &planned.event) {
        ("calendar_add", _) => {
            let url = format!("{base}/me/calendars/{}/events", planned.calendar);
            outlook::send(api, Method::POST, &url, body, None, &[PREFER_IMMUTABLE]).await
        }
        ("calendar_update", Some(id)) => {
            let url = event_url(api, &planned.calendar, id);
            outlook::send(api, Method::PATCH, &url, body, etag, &[PREFER_IMMUTABLE]).await
        }
        ("calendar_delete", Some(id)) => {
            outlook::send(api, Method::DELETE, &event_url(api, &planned.calendar, id), None, etag, &[]).await
        }
        _ => return tool_text("unknown event id (see calendar_list)", true),
    };
    match done {
        Ok(answer) => {
            tracing::info!(tool = planned.tool, "outlook: calendar written");
            let title = crate::archives::defused(&planned.title);
            match planned.tool {
                "calendar_add" => tool_text(
                    &format!(
                        "Added to {label}: id {} · {} · {title}. The person sees it in their calendar.",
                        event_ref(planned.calendar_n, outlook::short_id(config, account, &text(&answer, "id"))),
                        planned.when
                    ),
                    false,
                ),
                "calendar_update" => tool_text(&format!("Changed in {label}: {} · {title}.", planned.when), false),
                _ => tool_text(&format!("Deleted from {label}."), false),
            }
        }
        Err(fault) => fault_result(&fault),
    }
}

/// The card's sentence, in the agent's language; `account`: the account's address, said when the place keeps several.
pub fn sentence(lang: &str, tool: &str, shown: &str, title: &str, account: Option<&str>) -> String {
    use crate::person_ok::pick;
    let said = match tool {
        "calendar_add" => pick(
            lang,
            "{n} veut ajouter un événement à ton agenda Outlook",
            "{n} wants to add an event to your Outlook calendar",
            "{n} möchte einen Termin in deinen Outlook-Kalender eintragen",
            "{n} quiere añadir un evento a tu calendario de Outlook",
        ),
        "calendar_update" => pick(
            lang,
            "{n} veut modifier un événement de ton agenda Outlook",
            "{n} wants to change an event in your Outlook calendar",
            "{n} möchte einen Termin in deinem Outlook-Kalender ändern",
            "{n} quiere cambiar un evento de tu calendario de Outlook",
        ),
        _ => pick(
            lang,
            "{n} veut supprimer un événement de ton agenda Outlook",
            "{n} wants to delete an event from your Outlook calendar",
            "{n} möchte einen Termin aus deinem Outlook-Kalender löschen",
            "{n} quiere borrar un evento de tu calendario de Outlook",
        ),
    }
    .replace("{n}", shown);
    let said = match account {
        Some(address) => format!("{said} ({})", crate::archives::defused(address)),
        None => said,
    };
    let title = crate::cards::quoted(title, 120);
    match (title.is_empty(), lang.starts_with("fr")) {
        (true, _) => said,
        (false, true) => format!("{said} : « {title} »"),
        (false, false) => format!("{said}: « {title} »"),
    }
}

pub const WRITE_REFUSED: &str = "Your person said no: their Outlook calendar is not changed. Tell them plainly; don't \
ask again unless they tell you to.";
pub const WRITE_NO_ANSWER: &str = "Your person did not answer in time: nothing was changed in their Outlook calendar. \
Tell them their « Oui » waits in the app, and call again once they say so.";
pub const WRITE_HELD: &str = "Your person is not here now (this turn runs on its own): a card asks them before their \
Outlook calendar is changed; it was not refused. When they say yes you will be told: then make the same call again. \
End with your report saying what waits for their OK.";

/// A calendar write of `agent` for Outlook's calendars: checked there, asked on the person's card, done on « Oui ».
/// None: not for Outlook (another calendar takes it).
pub async fn write_call(state: &AppState, agent: &AgentConfig, name: &str, arguments: &Dict) -> Option<Dict> {
    if !matches!(name, "calendar_add" | "calendar_update" | "calendar_delete") {
        return None;
    }
    let config = state.config();
    let accounts = outlook::accounts_with(&config, &agent.name, "calendar");
    if accounts.is_empty() {
        return tk::str_or_empty(arguments.get("id"))
            .trim()
            .starts_with(ID_PREFIX)
            .then(|| tool_text("unknown event id (see calendar_list)", true));
    }
    let (c, a) = (config.clone(), agent.name.clone());
    let (others, elsewhere) = tokio::task::spawn_blocking(move || {
        let phone = crate::calendar::enabled(&c, &a);
        let sources = crate::caldav::sources(&c, &a);
        let mut elsewhere: Vec<String> = Vec::new();
        if phone {
            let data = crate::calendar::read(&c);
            let listed = data.get("calendars").and_then(Value::as_array).cloned().unwrap_or_default();
            elsewhere.extend(listed.iter().map(|x| text(x, "title")));
        }
        for source in &sources {
            for calendar in &source.calendars {
                elsewhere.push(calendar.title.clone());
                elsewhere.push(format!("{} ({})", calendar.title, source.label));
            }
        }
        (phone || !sources.is_empty() || crate::google_calendar::offered(&c, &a), elsewhere)
    })
    .await
    .ok()?;
    let addresses: Vec<String> = accounts.iter().filter_map(|a| a.link.account.clone()).collect();
    let aim = aim(name, arguments, others, &addresses, &elsewhere);
    if aim == Aim::No {
        return None;
    }
    let maybe = aim == Aim::IfNamed;
    // the account: the one of the event's id, else the one the add names (its address), else each in turn (the first
    // one's calendars first) until one has the calendar named
    let candidates: Vec<AccountId> = if name == "calendar_add" {
        let wanted = tk::str_or_empty(arguments.get("calendar"));
        let named: Vec<AccountId> = accounts
            .iter()
            .filter(|a| a.link.account.as_deref().is_some_and(|x| names_account(&wanted, x)))
            .map(|a| a.id)
            .collect();
        if named.is_empty() { accounts.iter().map(|a| a.id).collect() } else { named }
    } else {
        let unknown = || Some(tool_text("unknown event id (see calendar_list)", true));
        let Some((n, _)) = split_ref(&tk::str_or_empty(arguments.get("id"))) else { return unknown() };
        match outlook::long_id(&config, n) {
            Some((account, _)) if accounts.iter().any(|a| a.id == account) => vec![account],
            _ => return unknown(),
        }
    };
    let mut chosen: Option<(AccountId, Api, Calendar)> = None;
    let mut last_fault: Option<Fault> = None;
    for account in candidates {
        let api = match outlook::signed_in(&config, account).await {
            Ok(api) => api,
            Err(fault) => {
                last_fault = Some(fault);
                continue;
            }
        };
        let found = match calendars(&config, account, &api).await {
            Ok(found) => found,
            Err(fault) => {
                last_fault = Some(fault);
                continue;
            }
        };
        let address = outlook::address(&config, account);
        let calendar = if name == "calendar_add" {
            pick(&found, &tk::str_or_empty(arguments.get("calendar")), address.as_deref()).cloned()
        } else {
            let (n, _) = split_ref(&tk::str_or_empty(arguments.get("id")))?;
            found.iter().find(|c| c.n == n).cloned()
        };
        if let Some(calendar) = calendar {
            chosen = Some((account, api, calendar));
            break;
        }
    }
    let Some((account, api, calendar)) = chosen else {
        return match (maybe, last_fault) {
            (true, _) => None,
            (false, Some(fault)) => Some(fault_result(&fault)),
            (false, None) if name == "calendar_add" => Some(tool_text(NO_SUCH_CALENDAR, true)),
            (false, None) => Some(tool_text("unknown event id (see calendar_list)", true)),
        };
    };
    if !calendar.writable {
        return Some(tool_text(READ_ONLY, true));
    }
    let planned = match plan(&config, account, &api, name, arguments, &calendar).await {
        Ok(planned) => planned,
        Err(said) => return Some(said),
    };
    let lang = crate::mail_tidy::language(&config, &agent.name);
    let shown = crate::mail_tidy::shown_agent(&config, &agent.name);
    let label = outlook::label(&config, account);
    let mut detail = planned.when.clone();
    if !planned.location.is_empty() {
        detail += &format!(" · {}", crate::cards::quoted(&planned.location, 120));
    }
    let place = match &planned.calendar_name {
        Some(other) => format!("« {} » · {label}", crate::cards::quoted(other, 60)),
        None => label.clone(),
    };
    detail += &format!(" ({place})\n{}", crate::cards::quoted(&planned.title, 300));
    let delete = planned.tool == "calendar_delete";
    let address = outlook::address(&config, account);
    let named = address.as_deref().filter(|_| outlook::several(&config));
    let summary = sentence(&lang, planned.tool, &shown, &planned.title, named);
    // adds and changes in the account's default calendar may be allowed for good (its own key, each account's apart);
    // deletions and another calendar: asked each time
    let may_always = !delete && planned.calendar_name.is_none();
    let key = outlook::always_key(account);
    if may_always && crate::approval_level::granted(&config, &agent.name, &key) {
        let session = crate::runs::CALL_SESSION.try_with(|s| s.clone()).ok().flatten().filter(|s| !s.is_empty());
        crate::approval_level::record_always(&config, &agent.name, crate::person_ok::CALENDAR_WRITE, &summary, session);
        tracing::info!(agent = %agent.name, tool = planned.tool, account, "outlook calendar: always allowed by the person");
        return Some(apply(&config, account, &api, &planned).await);
    }
    let card = crate::person_ok::Card {
        kind: if delete { crate::person_ok::CALENDAR_DELETE } else { crate::person_ok::CALENDAR_WRITE_OUTLOOK },
        tool: planned.tool,
        command: planned.command(),
        summary,
        detail,
        always: if may_always { crate::approval_level::always_button(&lang, &key) } else { None },
        // another account's « Toujours »: under its own key (outlook.rs `always_key`)
        always_key: (may_always && key != outlook::ALWAYS_KEY).then(|| key.clone()),
    };
    use crate::person_ok::Answer;
    let refused = |a: &'static str, b: &'static str| if delete { a } else { b };
    match crate::person_ok::ask(state, agent, card).await {
        Answer::Once | Answer::Always => {
            // the account, the agenda and the agent still there (review C4): else nothing written
            let now = state.config();
            if let Err(why) = outlook::still_allowed(&now, &agent.name, "calendar", account, address.as_deref()) {
                return Some(tool_text(why, true));
            }
            match outlook::signed_in(&now, account).await {
                Ok(fresh) => Some(apply(&now, account, &fresh, &planned).await),
                Err(fault) => Some(fault_result(&fault)),
            }
        }
        Answer::Deny => Some(tool_text(refused(crate::calendar::DELETE_REFUSED, WRITE_REFUSED), true)),
        Answer::NoAnswer => Some(tool_text(refused(crate::calendar::DELETE_NO_ANSWER, WRITE_NO_ANSWER), true)),
        Answer::Held => Some(tool_text(refused(crate::calendar::DELETE_HELD, WRITE_HELD), false)),
    }
}
