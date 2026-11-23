//! A bot check that still blocks an agent's visible browser, told to its person (antibot-guard, 22/11;
//! passation-pro/briefs/enquete-antibot-1122.md). The browser's proxy (browser_mcp.rs `hand_over`) writes
//! `challenge.json` beside the browser's `busy` file (`{agent, site, kind, at}`) and its call waits for the person's
//! hand; this loop turns each note into ONE Boîte line with a push, in the agent's language: « Une vérification
//! anti-robots bloque decathlon.fr. Prends la main dans le Bureau de Quotidien pour la passer, puis rends-la :
//! Quotidien reprendra. », at most once per agent and site in `KEEP_SECONDS`. The person takes the hand from the
//! conversation's browser card or the Bureau (« Prendre la main »), the existing way; nothing new to answer.
//!
//! Only the agents' own browsers: a task's own browser stops at once (nobody takes it over), its report says it.
//!
//! Review 1 (22/11): where the phone exit can carry the site, the line offers the person's phone beside the hand
//! (`phone_line`, Alex's request, its wording proposed to him); and when the agent stopped waiting for the hand (its
//! wait is bounded by Hermes' time), one more line, without a push, says so (`stopped_line`): « reprendra » was only
//! true while it waited.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::app::AppState;
use crate::config::Config;
use crate::outbox::BoiteMessage;

/// How often the browsers' folders are looked at.
pub const EVERY: Duration = Duration::from_secs(2);
/// One line per agent and site in so long at most.
pub const KEEP_SECONDS: f64 = 1800.0;
/// A note is at most so long (it is the bridge's own proxy's, a few dozen bytes).
const NOTE_MAX: u64 = 4096;

/// The Boîte line's title and words, in `lang` (« fr », « en », « de », « es »; another: English).
pub fn line(lang: &str, agent: &str, site: &str) -> (&'static str, String) {
    match crate::names::names(lang).lang {
        "fr" => (
            "Vérification anti-robots",
            format!(
                "Une vérification anti-robots bloque {site}. Prends la main dans le Bureau de {agent} pour la passer, \
                 puis rends-la : {agent} reprendra."
            ),
        ),
        "de" => (
            "Bot-Prüfung",
            format!(
                "Eine Bot-Prüfung blockiert {site}. Übernimm den Desktop von {agent}, um sie zu bestehen, und gib ihn \
                 dann zurück: {agent} macht weiter."
            ),
        ),
        "es" => (
            "Verificación antibots",
            format!(
                "Una verificación antibots bloquea {site}. Toma el control del Escritorio de {agent} para pasarla y \
                 luego devuélvelo: {agent} seguirá."
            ),
        ),
        _ => (
            "Bot check",
            format!(
                "A bot check is blocking {site}. Take over {agent}'s Desktop to pass it, then hand it back: {agent} will \
                 carry on."
            ),
        ),
    }
}

/// `line` when the person's phone can carry the site too (the phone exit, Alex 22/11: offered beside the hand).
/// Proposed to Alex (review 1 of antibot-guard), shown only where the phone exit is on.
pub fn phone_line(lang: &str, agent: &str, site: &str) -> (&'static str, String) {
    let title = line(lang, agent, site).0;
    let text = match crate::names::names(lang).lang {
        "fr" => format!(
            "Une vérification anti-robots bloque {site}. Prends la main dans le Bureau de {agent} pour la passer, ou \
             fais passer {site} par la connexion de ton téléphone : {agent} reprendra."
        ),
        "de" => format!(
            "Eine Bot-Prüfung blockiert {site}. Übernimm den Desktop von {agent}, um sie zu bestehen, oder leite \
             {site} über die Verbindung deines Telefons: {agent} macht weiter."
        ),
        "es" => format!(
            "Una verificación antibots bloquea {site}. Toma el control del Escritorio de {agent} para pasarla, o haz \
             pasar {site} por la conexión de tu teléfono: {agent} seguirá."
        ),
        _ => format!(
            "A bot check is blocking {site}. Take over {agent}'s Desktop to pass it, or let {site} go through your \
             phone's connection: {agent} will carry on."
        ),
    };
    (title, text)
}

/// The agent stopped waiting for the person's hand (`hand_over`'s wait is bounded by Hermes' time): « reprendra » is
/// no longer true, the person tells it to go on (review 1 of antibot-guard). No push: the first line had one.
pub fn stopped_line(lang: &str, agent: &str, site: &str) -> (&'static str, String) {
    let title = line(lang, agent, site).0;
    let text = match crate::names::names(lang).lang {
        "fr" => format!("{agent} n'attend plus pour {site} : passe la vérification quand tu veux, puis dis-lui de reprendre."),
        "de" => format!("{agent} wartet nicht mehr auf {site}: Bestehe die Prüfung, wann du willst, und sag {agent} dann, dass es weitermachen soll."),
        "es" => format!("{agent} ya no espera por {site}: pasa la verificación cuando quieras y luego dile que siga."),
        _ => format!("{agent} stopped waiting for {site}: pass the check whenever you like, then tell {agent} to carry on."),
    };
    (title, text)
}

/// A Boîte line to post, and whether it is pushed.
#[derive(Debug, Clone)]
pub struct Told {
    pub agent: String,
    pub message: BoiteMessage,
    pub push: bool,
}

/// A site's name as the proxy wrote it: letters, digits, dots and dashes only (None: not one).
fn plain_site(raw: &str) -> Option<String> {
    let site = raw.trim().to_ascii_lowercase();
    let ok = !site.is_empty()
        && site.len() <= 253
        && site.contains('.')
        && site.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then_some(site)
}

/// The notes of the agents' browsers, each taken once (the file removed): the lines to post, by agent, not posted for
/// that agent and site within `KEEP_SECONDS` (`seen`: agent and site → when). Never through a link.
pub fn take(config: &Config, seen: &mut HashMap<(String, String), f64>, now: f64) -> Vec<(String, BoiteMessage)> {
    take_lines(config, seen, now).into_iter().map(|t| (t.agent, t.message)).collect()
}

/// `take`, each line with whether it is pushed: the first of a check (`line`, or `phone_line` when the person's phone
/// is offered too), then, once the agent stopped waiting, `stopped_line` without a push.
pub fn take_lines(config: &Config, seen: &mut HashMap<(String, String), f64>, now: f64) -> Vec<Told> {
    seen.retain(|_, at| now - *at < KEEP_SECONDS);
    let mut out = Vec::new();
    // the first note of each agent's browser, then the one saying it stopped waiting (a file of its own: review 2)
    let notes = config.agents.values().flat_map(|agent| {
        [crate::browser_mcp::CHECK_FILE, crate::browser_mcp::CHECK_STOPPED_FILE]
            .map(|name| (agent, crate::browsers::folder(config, &agent.name).join(name)))
    });
    for (agent, path) in notes {
        let Ok(meta) = path.symlink_metadata() else { continue };
        let note = if meta.is_file() && meta.len() <= NOTE_MAX { std::fs::read_to_string(&path).ok() } else { None };
        let _ = std::fs::remove_file(&path);
        let Some(Value::Object(note)) = note.and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let Some(site) = note.get("site").and_then(Value::as_str).and_then(plain_site) else { continue };
        let stopped = note.get("stopped").and_then(Value::as_bool).unwrap_or(false);
        let phone = note.get("phone").and_then(Value::as_bool).unwrap_or(false);
        // the stop has its own once-only key: it follows the first line of the same check
        let key = (agent.name.clone(), if stopped { format!("{site} stopped") } else { site.clone() });
        if seen.contains_key(&key) {
            continue;
        }
        seen.insert(key, now);
        let shown = if agent.display_name.is_empty() { agent.name.clone() } else { agent.display_name.clone() };
        let lang = crate::names::agent_language(config, &agent.name);
        let (title, text) = match (stopped, phone) {
            (true, _) => stopped_line(&lang, &shown, &site),
            (false, true) => phone_line(&lang, &shown, &site),
            (false, false) => line(&lang, &shown, &site),
        };
        let kind = note.get("kind").and_then(Value::as_str).unwrap_or("");
        tracing::info!(agent = %agent.name, site = %site, kind = %kind, stopped, phone, "a bot check handed to the person: Boîte line");
        out.push(Told {
            agent: agent.name.clone(),
            message: BoiteMessage { title: Some(title.to_string()), ..BoiteMessage::new(text) },
            push: !stopped,
        });
    }
    out
}

/// The notes looked at every `EVERY`, each posted with a push.
pub async fn run_forever(state: AppState) {
    let mut seen: HashMap<(String, String), f64> = HashMap::new();
    loop {
        tokio::time::sleep(EVERY).await;
        let config = state.config();
        let (c, mut s) = (config.clone(), std::mem::take(&mut seen));
        let Ok((lines, back)) = tokio::task::spawn_blocking(move || {
            let lines = take_lines(&c, &mut s, crate::py::time_time());
            (lines, s)
        })
        .await
        else {
            continue;
        };
        seen = back;
        for Told { agent: name, message, push } in lines {
            let Some(agent) = config.agent(Some(&name)) else { continue };
            if let Err(exc) = state.outbox.ingest(agent, message, push).await {
                tracing::warn!(agent = %name, error = %format!("{exc:#}"), "a bot check's Boîte line not stored");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(dir: &std::path::Path, lang: &str) -> Config {
        crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.join("data").to_string_lossy(),
                    "browser": {"enabled": true, "root": dir.join("browsers").to_string_lossy(), "language": lang},
                    "agents": {"daily": {"display_name": "Quotidien", "hermes_url": "http://127.0.0.1:8642",
                                         "hermes_key": "h".repeat(32)}}}),
            None,
        )
        .unwrap()
    }

    fn note(dir: &std::path::Path, site: &str) -> std::path::PathBuf {
        let folder = dir.join("browsers").join("daily");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(crate::browser_mcp::CHECK_FILE);
        let body = json!({"agent": "daily", "site": site, "kind": "Cloudflare challenge", "at": 1.0});
        std::fs::write(&path, body.to_string()).unwrap();
        path
    }

    #[test]
    fn one_line_per_agent_and_site_then_none_for_half_an_hour() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path(), "fr");
        let mut seen = HashMap::new();
        let path = note(dir.path(), "decathlon.fr");
        let lines = take(&config, &mut seen, 1000.0);
        assert_eq!(lines.len(), 1);
        let (agent, message) = &lines[0];
        assert_eq!(agent, "daily");
        assert_eq!(message.title.as_deref(), Some("Vérification anti-robots"));
        assert_eq!(
            message.text,
            "Une vérification anti-robots bloque decathlon.fr. Prends la main dans le Bureau de Quotidien pour la \
             passer, puis rends-la : Quotidien reprendra."
        );
        assert!(!path.exists(), "taken once");
        // the same site again within half an hour: nothing; another site: its line
        note(dir.path(), "decathlon.fr");
        assert!(take(&config, &mut seen, 1000.0 + 60.0).is_empty());
        note(dir.path(), "leboncoin.fr");
        assert_eq!(take(&config, &mut seen, 1000.0 + 61.0).len(), 1);
        note(dir.path(), "decathlon.fr");
        assert_eq!(take(&config, &mut seen, 1000.0 + KEEP_SECONDS + 1.0).len(), 1);
        // nothing there, or a note that isn't one: nothing
        assert!(take(&config, &mut seen, 5000.0).is_empty());
        note(dir.path(), "x; rm -rf /");
        assert!(take(&config, &mut seen, 5001.0).is_empty());
    }

    #[test]
    fn in_the_agents_language() {
        let (title, text) = line("en", "Daily", "decathlon.fr");
        assert_eq!(title, "Bot check");
        assert_eq!(
            text,
            "A bot check is blocking decathlon.fr. Take over Daily's Desktop to pass it, then hand it back: Daily will \
             carry on."
        );
        assert!(line("de", "Daily", "x.de").1.starts_with("Eine Bot-Prüfung blockiert x.de."));
        assert!(line("es", "Daily", "x.es").1.starts_with("Una verificación antibots bloquea x.es."));
        assert_eq!(line("it", "Daily", "x.it").0, "Bot check");
    }

    /// Review 1 of antibot-guard and Alex (22/11): the person's phone offered beside the hand when the phone exit can
    /// carry the site; and when the agent stopped waiting, the Boîte says so (« reprendra » was only true while it
    /// waited), without a second push.
    #[test]
    fn the_phone_beside_the_hand_and_the_agent_that_stopped_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path(), "fr");
        let mut seen = HashMap::new();
        let folder = dir.path().join("browsers").join("daily");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(crate::browser_mcp::CHECK_FILE);
        let write = |phone: bool, stopped: bool| {
            let body = json!({"agent": "daily", "site": "decathlon.fr", "kind": "reCAPTCHA", "at": 1.0,
                              "phone": phone, "stopped": stopped});
            std::fs::write(&path, body.to_string()).unwrap();
        };
        write(true, false);
        let lines = take_lines(&config, &mut seen, 1000.0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].push);
        assert_eq!(
            lines[0].message.text,
            "Une vérification anti-robots bloque decathlon.fr. Prends la main dans le Bureau de Quotidien pour la \
             passer, ou fais passer decathlon.fr par la connexion de ton téléphone : Quotidien reprendra."
        );
        // the wait over, nobody came: one more line, no push; once. Its own file (review 2): both lines when both
        // notes wait for the bridge
        let stopped_path = folder.join(crate::browser_mcp::CHECK_STOPPED_FILE);
        let write = |phone: bool, stopped: bool| {
            let body = json!({"agent": "daily", "site": "decathlon.fr", "kind": "reCAPTCHA", "at": 1.0,
                              "phone": phone, "stopped": stopped});
            std::fs::write(if stopped { &stopped_path } else { &path }, body.to_string()).unwrap();
        };
        write(true, true);
        let lines = take_lines(&config, &mut seen, 1600.0);
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].push);
        assert_eq!(
            lines[0].message.text,
            "Quotidien n'attend plus pour decathlon.fr : passe la vérification quand tu veux, puis dis-lui de reprendre."
        );
        write(true, true);
        assert!(take_lines(&config, &mut seen, 1601.0).is_empty());
        let mut fresh = HashMap::new();
        write(false, false);
        write(false, true);
        let both = take_lines(&config, &mut fresh, 5000.0);
        assert_eq!(both.iter().map(|t| t.push).collect::<Vec<_>>(), vec![true, false]);
        assert!(stopped_line("en", "Daily", "x.com").1.starts_with("Daily stopped waiting for x.com"));
        assert!(phone_line("en", "Daily", "x.com").1.contains("phone's connection"));
    }

    #[test]
    fn never_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path(), "fr");
        let folder = dir.path().join("browsers").join("daily");
        std::fs::create_dir_all(&folder).unwrap();
        let elsewhere = dir.path().join("elsewhere.json");
        std::fs::write(&elsewhere, json!({"site": "decathlon.fr"}).to_string()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, folder.join(crate::browser_mcp::CHECK_FILE)).unwrap();
        assert!(take(&config, &mut HashMap::new(), 1.0).is_empty());
        assert!(elsewhere.exists());
    }
}
