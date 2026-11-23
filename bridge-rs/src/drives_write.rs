//! Writing in « Documents en ligne » (Alex 23/11, « Oui, ajoute », after trying Dropbox on his aibox: his agent read
//! but could not make a file). One tool, `drive_write`, for an agent the person lets write in a folder (drives.rs
//! `Folder::writers`, « {Agent} peut y écrire », off by default; the whole drive's too, « Tout mon Dropbox »):
//!
//! - a NEW file in that folder (or a sub-folder of it): `folder` + `name`, with `text` (plain or markdown) or `file`
//!   (one of its own files, as sent_files.rs `open_working` opens them: its working places only, name by name
//!   without a link, never hidden nor secret-looking; or one of its own pages, `/bipagents/pages/<itself>/<id>.html`).
//!   A name already there is refused: never written over;
//! - a NEW VERSION of a file there (`replace`): Dropbox `upload` in `update` mode on the revision read, Google a new
//!   revision (the one before kept for good when Google takes it), OneDrive `PUT …/content` with `If-Match`; the
//!   service keeps the old version in its history. A Google Docs/Sheets/Slides document: refused (a new file instead).
//!
//! Never: deleting, moving, renaming; anything outside the folders given for writing (checked as the reads are: by
//! the parents, or Dropbox's path read again by its id); a name that is a path (`/`, `\`, `..`, a hidden `.x`, a
//! control or direction character, Windows' forbidden ones); a Google shared drive; a folder or file shared with other
//! people (Google `shared`, OneDrive's `shared` facet, Dropbox `sharing_info`: the file would show to them, a way out
//! for what the agent read); more than [`WRITE_MAX_BYTES`].
//!
//! The person's say (person_ok.rs, a `bip-ok-` card « Oui / Non »): « Enregistrer « {file} » dans {service} ›
//! {folder} ? ». A new file is `drive_write`, not critical (approval_level.rs): under « Seulement le critique » it is
//! made at once and logged as a « Validé pour toi » line whose « Annuler » puts that new file in the service's bin
//! (only while it is unchanged: [`undo_file`]). A new version is `drive_replace`, critical: always asked. After the
//! « Oui » everything is checked again before writing: a folder taken back (or its switch turned off) while the card
//! waited, a file changed meanwhile: nothing written.
//!
//! Google stays on `drive.file`: it lets the app create files in a folder picked in Google's window and change files
//! it made or that were picked. A file only inside a picked folder: not documented by Google; tried, and its refusal
//! said plainly (« pick that file in the app »). No wider scope is ever asked.
//!
//! No Python counterpart.

use std::io::Read;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::drives::{self, Api, Build, Fault, Folder, Item, Provider, ROOT_ID};
use crate::tools_knowledge::{tool_failure, tool_text};
use crate::{py, Dict};

pub const TOOL: &str = drives::WRITE_TOOL;
/// A new file (approval_level.rs: not critical, undone by putting it in the bin).
pub const KIND_CREATE: &str = "drive_write";
/// A new version of a file (critical: always asked).
pub const KIND_REPLACE: &str = "drive_replace";
/// The largest file written (as the largest one read).
pub const WRITE_MAX_BYTES: u64 = drives::FILE_MAX_BYTES;
/// Characters of a new file's name.
pub const NAME_MAX: usize = 120;
/// Characters of the text shown on the card.
const SHOWN_CHARS: usize = 600;

pub fn tool() -> Value {
    let reference = "a reference as drive_folders, drive_list or drive_search gave it";
    json!({
        "name": TOOL,
        "description": "Save a file in your person's online documents (Google Drive, OneDrive, Dropbox), only in a \
    folder they let you write in (drive_folders says « you may write in it »): a NEW file (`folder` and `name`, with \
    `text` or `file`), or a NEW VERSION of a file there (`replace`, with `text` or `file`; the old version stays in the \
    service's history). Never deletes, moves or renames anything, never writes over a file. Your person sees it on a card \
    and says yes first (a new file may go through at once when they chose so).",
        "inputSchema": {"type": "object", "properties": {
            "folder": {"type": "string", "description": format!("the folder to save in: {reference}")},
            "name": {"type": "string", "description": "the new file's name, e.g. « Compte rendu.md »", "maxLength": NAME_MAX},
            "text": {"type": "string", "description": "the content, as text (markdown or plain)"},
            "file": {"type": "string", "description": "or one of your files, as you see it (e.g. /envoi/devis.pdf), or a page's MEDIA: path"},
            "replace": {"type": "string", "description": format!("instead of `folder`: the file to add a new version to: {reference}")},
        }},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": true},
    })
}

// -- what is asked ----------------------------------------------------------------------------------------------------

fn arg(arguments: &Dict, key: &str) -> String {
    arguments.get(key).map(py::py_str).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// A new file's name, or why not: one part of a path, nothing hidden, nothing a drive or a computer would take for
/// something else.
pub fn clean_name(raw: &str) -> Result<String, &'static str> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("give the new file's `name`");
    }
    if name.chars().count() > NAME_MAX {
        return Err("the name is too long (120 characters at most)");
    }
    if name.trim_matches('.').is_empty() {
        return Err("a name, not a path");
    }
    if name.contains(['/', '\\']) {
        return Err("a name, not a path: no « / » nor « \\ » (save it in the folder itself)");
    }
    let direction = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    if name.chars().any(|c| c.is_control() || direction(c)) {
        return Err("no control characters in a name");
    }
    if name.contains([':', '*', '?', '"', '<', '>', '|']) {
        return Err("none of : * ? \" < > | in a name");
    }
    if name.starts_with('.') {
        return Err("no hidden file (a name starting with « . »)");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("a name can't end with « . »");
    }
    Ok(name.to_string())
}

fn extension(name: &str) -> Option<&str> {
    name.rsplit_once('.').filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty()).map(|(_, e)| e)
}

/// The file's name: the one asked (its extension added from the content when it has none: `.md` for a text).
pub fn name_with_extension(asked: &str, own: Option<&str>) -> String {
    if extension(asked).is_some() {
        return asked.to_string();
    }
    match own.and_then(extension) {
        Some(ext) => format!("{asked}.{ext}"),
        None if own.is_none() => format!("{asked}.md"),
        None => asked.to_string(),
    }
}

/// What is written.
#[derive(Debug, Clone)]
pub struct Content {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// A text given in the call (the card shows its beginning).
    pub text: Option<String>,
    /// The agent's file's own name (`file`).
    pub own_name: Option<String>,
}

fn mime_of(name: &str, text: bool) -> String {
    match extension(name).map(str::to_ascii_lowercase).as_deref() {
        Some("md") | Some("markdown") => "text/markdown".into(),
        Some("txt") => "text/plain".into(),
        _ => crate::media::content_type_of(name).unwrap_or_else(|| {
            if text {
                "text/plain".into()
            } else {
                "application/octet-stream".into()
            }
        }),
    }
}

/// One of the agent's own pages (`/bipagents/pages/<agent>/<id>.html`), opened without following a link.
fn open_page(config: &Config, agent: &str, path: &str) -> Option<Result<(String, std::fs::File), String>> {
    let rest = path.strip_prefix(crate::pages::PREFIX)?.strip_prefix('/')?;
    let Some((owner, file)) = rest.split_once('/') else { return Some(Err("not a page of yours".into())) };
    let page = file.strip_suffix(".html").unwrap_or("");
    if owner != agent || !crate::pages::ID.is_match(page) || page.ends_with('\n') {
        return Some(Err("not a page of yours".into()));
    }
    use std::os::unix::fs::OpenOptionsExt;
    let at = crate::pages::root(config).join(owner).join(file);
    Some(
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&at)
            .map(|f| (file.to_string(), f))
            .map_err(|_| "no such page of yours".to_string()),
    )
}

/// The content of the call: its `text`, or the agent's `file` (blocking: files).
pub fn content_of(config: &Config, agent: &AgentConfig, text: &str, file: &str) -> Result<Content, String> {
    match (text.is_empty(), file.is_empty()) {
        (false, false) => return Err("give `text` or `file`, not both".into()),
        (true, true) => return Err("give the content: `text`, or `file` (one of your files)".into()),
        _ => {}
    }
    if !text.is_empty() {
        if text.len() as u64 > WRITE_MAX_BYTES {
            return Err(too_large());
        }
        return Ok(Content {
            bytes: text.as_bytes().to_vec(),
            mime: String::new(),
            text: Some(text.to_string()),
            own_name: None,
        });
    }
    let path = file.trim().trim_start_matches("MEDIA:").trim();
    let path = path.strip_prefix("file://").unwrap_or(path);
    let (seen, mut handle) = match open_page(config, &agent.name, path) {
        Some(found) => found?,
        None => crate::sent_files::open_working(config, agent, path).map_err(str::to_string)?,
    };
    let meta = handle.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > WRITE_MAX_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&mut handle).take(WRITE_MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > WRITE_MAX_BYTES {
        return Err(too_large());
    }
    let own = seen.rsplit('/').next().unwrap_or(&seen).to_string();
    Ok(Content { bytes, mime: String::new(), text: None, own_name: Some(own) })
}

fn too_large() -> String {
    format!("too large to save there (over {} MB)", WRITE_MAX_BYTES / (1024 * 1024))
}

// -- the words ----------------------------------------------------------------------------------------------------------

fn pick5<'a>(lang: &str, fr: &'a str, en: &'a str, de: &'a str, es: &'a str, nl: &'a str) -> &'a str {
    match lang.split(['-', '_']).next().unwrap_or("") {
        "nl" => nl,
        other => crate::person_ok::pick(other, fr, en, de, es),
    }
}

/// « 12 ko », « 1,4 Mo » (fr), « 12 KB » elsewhere.
pub fn size_text(lang: &str, bytes: u64) -> String {
    let french = lang.starts_with("fr");
    let comma = !lang.starts_with("en");
    if bytes < 1024 * 1024 {
        return format!("{} {}", bytes.div_ceil(1024).max(1), if french { "ko" } else { "KB" });
    }
    let mb = format!("{:.1}", bytes as f64 / 1_048_576.0);
    format!("{} {}", if comma { mb.replace('.', ",") } else { mb }, if french { "Mo" } else { "MB" })
}

/// The card's sentence (to validate by Alex: briefs/status-drive-write.md).
pub fn card_sentence(lang: &str, replace: bool, file: &str, place: &str) -> String {
    let said = if replace {
        pick5(
            lang,
            "Nouvelle version de « {f} » dans {p} ?",
            "New version of « {f} » in {p}?",
            "Neue Version von « {f} » in {p}?",
            "¿Nueva versión de « {f} » en {p}?",
            "Nieuwe versie van « {f} » in {p}?",
        )
    } else {
        pick5(
            lang,
            "Enregistrer « {f} » dans {p} ?",
            "Save « {f} » in {p}?",
            "« {f} » in {p} speichern?",
            "¿Guardar « {f} » en {p}?",
            "« {f} » opslaan in {p}?",
        )
    };
    said.replace("{f}", file).replace("{p}", place)
}

/// What the card shows under it: the text's beginning, or what kind of file; a new version says where the old stays.
pub fn card_detail(lang: &str, replace: bool, label: &str, content: &Content) -> String {
    let mut out = Vec::new();
    if replace {
        out.push(
            pick5(
                lang,
                "L'ancienne version reste dans l'historique de {s}.",
                "The old version stays in {s}'s history.",
                "Die alte Version bleibt im Verlauf von {s}.",
                "La versión anterior queda en el historial de {s}.",
                "De oude versie blijft in de geschiedenis van {s}.",
            )
            .replace("{s}", label),
        );
    }
    match &content.text {
        Some(text) => {
            let shown: String = text.chars().take(SHOWN_CHARS).collect();
            let cut = if text.chars().count() > SHOWN_CHARS { "…" } else { "" };
            out.push(format!("{}{cut}", shown.trim_end()));
        }
        None => {
            let size = size_text(lang, content.bytes.len() as u64);
            out.push(if replace {
                size
            } else {
                format!(
                    "{} · {size}",
                    pick5(lang, "Nouveau fichier", "New file", "Neue Datei", "Archivo nuevo", "Nieuw bestand")
                )
            });
        }
    }
    out.join("\n")
}

/// The « Validé pour toi » line of a new file made at once.
pub fn line_sentence(lang: &str, file: &str, place: &str) -> String {
    pick5(
        lang,
        "« {f} » enregistré dans {p}",
        "« {f} » saved in {p}",
        "« {f} » in {p} gespeichert",
        "« {f} » guardado en {p}",
        "« {f} » opgeslagen in {p}",
    )
    .replace("{f}", file)
    .replace("{p}", place)
}

// -- where it goes --------------------------------------------------------------------------------------------------------

/// The write, checked at the service.
pub struct Plan {
    pub api: Api,
    pub provider: Provider,
    /// The folder written into (a new file), or the file versioned.
    pub target: Item,
    pub replace: bool,
    /// The file's name.
    pub name: String,
    /// « Dropbox › Travail » (the whole drive: « Dropbox »).
    pub place: String,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("provider", &self.provider)
            .field("target", &self.target)
            .field("replace", &self.replace)
            .field("name", &self.name)
            .field("place", &self.place)
            .finish()
    }
}

/// Why not, for the agent (and, when the person can change it, the sentence for them).
fn refused(text: &str) -> Dict {
    tool_text(text, true)
}

/// `item` (or a folder above it, up to the folders given for writing) open to other people, or in a Google shared
/// drive: never written into.
async fn open_to_others(api: &Api, item: &Item, writable: &[Folder]) -> bool {
    if item.shared || item.shared_drive {
        return true;
    }
    // Dropbox says it of each item (`sharing_info`, a shared folder's content too); the others by their parents
    if api.provider() == Provider::Dropbox {
        return false;
    }
    let chosen: Vec<&str> = writable.iter().map(|f| f.id.as_str()).collect();
    if chosen.contains(&item.id.as_str()) {
        return false;
    }
    let mut queue = item.parents.clone();
    let mut seen: Vec<String> = Vec::new();
    while let Some(id) = queue.pop() {
        if seen.contains(&id) || seen.len() >= drives::HOPS_MAX {
            continue;
        }
        seen.push(id.clone());
        let Ok(parent) = api.item_with(&id, drives::WRITE_FIELDS).await else { continue };
        if parent.shared || parent.shared_drive {
            return true;
        }
        if !chosen.contains(&parent.id.as_str()) {
            queue.extend(parent.parents);
        }
    }
    false
}

/// The file named `name` in `folder`, when there is one (what the app may see of it).
async fn child_named(api: &Api, folder: &Item, name: &str) -> Result<Option<Item>, Fault> {
    let p = api.provider();
    match p {
        Provider::Google => {
            let q = format!(
                "'{}' in parents and name = '{}' and trashed = false",
                drives::google_quoted(&folder.id),
                drives::google_quoted(name)
            );
            let v =
                api.get(&format!("{}/files", api.ep.api), &[("q", q.as_str()), ("fields", "files(id,name)")]).await?;
            Ok(v["files"].as_array().and_then(|f| f.first()).map(|f| drives::item_of(p, f)))
        }
        Provider::Microsoft => {
            let url = format!(
                "{}/me/drive/items/{}:/{}:",
                api.ep.api,
                drives::path_segment(&folder.id),
                drives::path_segment(name)
            );
            match api.get(&url, &[]).await {
                Ok(v) => Ok(Some(drives::item_of(p, &v))),
                Err(Fault::NotFound) => Ok(None),
                Err(other) => Err(other),
            }
        }
        Provider::Dropbox => {
            let path = format!("{}/{name}", if folder.id == ROOT_ID { "" } else { folder.path.trim_end_matches('/') });
            match api.post(&format!("{}/files/get_metadata", api.ep.api), json!({"path": path})).await {
                Ok(v) => Ok(Some(drives::item_of(p, &v)).filter(|i| !i.trashed)),
                Err(Fault::NotFound) => Ok(None),
                Err(other) => Err(other),
            }
        }
    }
}

/// The name of the folder a file is in, for the card.
async fn parent_name(api: &Api, item: &Item) -> Option<String> {
    if api.provider() == Provider::Dropbox {
        let shown = if item.display.is_empty() { &item.path } else { &item.display };
        let (folder, _) = shown.rsplit_once('/')?;
        return folder.rsplit('/').next().filter(|n| !n.is_empty()).map(str::to_string);
    }
    let parent = api.item(item.parents.first()?).await.ok()?;
    Some(parent.name).filter(|n| !n.is_empty() && parent.id != ROOT_ID)
}

/// Where `agent` would write `asked` (a folder, or a file to version), checked at the service as for reading, then
/// for writing: the folder given to it for writing (its switch on), not open to others, the name free.
pub async fn plan(config: &Config, agent: &AgentConfig, asked: &str, replace: bool, name: &str) -> Result<Plan, Dict> {
    let Some((provider, id)) = drives::parse_reference(asked) else {
        return Err(refused("Give a reference as drive_folders, drive_list or drive_search gave it."));
    };
    let label = provider.label();
    let stored =
        drives::read_stored(config).map_err(|e| refused(&format!("The online documents can't be read now: {e}")))?;
    let mine = drives::of_provider(&drives::folders_of(&stored, &agent.name), provider);
    if mine.is_empty() {
        return Err(refused(&format!("You may read no folder of {label}.")));
    }
    let shown = crate::mail_tidy::shown_agent(config, &agent.name);
    let switch_off = || {
        tool_failure(
            &format!(
                "Your person hasn't let you write there: they switch on « {shown} peut y écrire » on that folder in the \
app (Documents en ligne › {label})."
            ),
            &format!(
                "Je ne peux pas écrire dans ce dossier : allume « {shown} peut y écrire » sur ce dossier dans l'app, \
« Documents en ligne » › {label}."
            ),
        )
    };
    let writable_kept: Vec<Folder> = mine.iter().filter(|f| f.writers.contains(&agent.name)).cloned().collect();
    if writable_kept.is_empty() {
        return Err(switch_off());
    }
    // a connection made without the write right (Dropbox before a writer, OneDrive with Files.Read): asked of the
    // person at once, never after a card
    if stored.drives.get(provider.id()).is_some_and(|l| l.state == "connected" && !drives::writable(provider, Some(l)))
    {
        return Err(drives::told(provider, label, &Fault::MissingScope));
    }
    let api = drives::access(config, provider).await.map_err(|fault| drives::told(provider, label, &fault))?;
    let target =
        api.item_with(&id, drives::WRITE_FIELDS).await.map_err(|fault| drives::told(provider, asked, &fault))?;
    if target.trashed || target.remote {
        return Err(refused(&format!("{asked} is not in a folder your person let you read.")));
    }
    if replace && target.folder {
        return Err(refused(&format!("{asked} is a folder: give `folder` and `name` to save a new file in it.")));
    }
    if !replace && !target.folder {
        return Err(refused(&format!(
            "{asked} is a file: to add a new version of it, give it as `replace`; to save a new file, give its folder."
        )));
    }
    if replace && target.mime.starts_with("application/vnd.google-apps.") {
        return Err(refused(
            "A Google Docs, Sheets or Slides document can't get a new version from here: save a new file instead.",
        ));
    }
    let writable = drives::current(&api, &writable_kept).await;
    if !drives::inside(&api, &target, &writable).await {
        let readable = drives::current(&api, &mine).await;
        if drives::inside(&api, &target, &readable).await {
            return Err(switch_off());
        }
        return Err(refused(&format!("{asked} is not in a folder your person let you read.")));
    }
    if open_to_others(&api, &target, &writable).await {
        return Err(refused(&format!(
            "{asked} is shared with other people (or in a shared drive): a file saved there would show to them. Not \
from here: save it in a folder only your person sees."
        )));
    }
    let name = if replace { target.name.clone() } else { name.to_string() };
    if !replace {
        match child_named(&api, &target, &name).await {
            Ok(Some(found)) => {
                return Err(refused(&format!(
                    "« {} » exists already there ({}): never written over. Add a new version of it (`replace`) if \
your person wants it changed, or choose another name.",
                    drives::defused(&name),
                    drives::reference(provider, &found.id)
                )))
            }
            Ok(None) => {}
            Err(fault) => return Err(drives::told(provider, &format!("{asked} can't be checked"), &fault)),
        }
    }
    let folder_name = if replace {
        parent_name(&api, &target).await
    } else if target.id == ROOT_ID || writable.iter().any(|f| f.is_root() && f.id == target.id) {
        None
    } else {
        Some(target.name.clone()).filter(|n| !n.is_empty())
    };
    let place = match folder_name {
        Some(folder) => format!("{label} › {folder}"),
        None => label.to_string(),
    };
    Ok(Plan { api, provider, target, replace, name, place })
}

// -- the services' writes -----------------------------------------------------------------------------------------------

/// JSON with every character past ASCII escaped (Dropbox's `Dropbox-API-Arg` header takes ASCII only).
pub fn ascii_json(value: &Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// The same scheme, host and port: a Google upload address is only followed there (the token never goes elsewhere).
fn same_origin(a: &str, b: &str) -> bool {
    match (url::Url::parse(a), url::Url::parse(b)) {
        (Ok(a), Ok(b)) => {
            a.scheme() == b.scheme()
                && a.host_str() == b.host_str()
                && a.port_or_known_default() == b.port_or_known_default()
        }
        _ => false,
    }
}

/// Google's resumable upload: the session asked (`method` POST for a new file, PATCH for a new version, with the
/// metadata), then the bytes sent to the address it gave, on the same host only.
async fn google_upload(
    api: &Api,
    method: reqwest::Method,
    url: String,
    metadata: Value,
    content: &Content,
    mime: &str,
) -> Result<Value, Fault> {
    let started = {
        let (url, metadata, mime) = (url.clone(), metadata.clone(), mime.to_string());
        let method = method.clone();
        api.send(Box::new(move |http: &reqwest::Client, token: &str| {
            http.request(method.clone(), &url)
                .bearer_auth(token)
                .query(&[("uploadType", "resumable"), ("fields", "id,name,mimeType,parents,headRevisionId,trashed")])
                .header("X-Upload-Content-Type", mime.clone())
                .json(&metadata)
        }) as Build<'_>)
            .await?
    };
    let location = started
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or(Fault::Unavailable)?;
    if !same_origin(&location, &api.ep.content) {
        tracing::warn!("drives: Google's upload address is on another host: not followed");
        return Err(Fault::Unavailable);
    }
    let (bytes, mime) = (content.bytes.clone(), mime.to_string());
    api.json(Box::new(move |http: &reqwest::Client, token: &str| {
        http.put(&location).bearer_auth(token).header("content-type", mime.clone()).body(bytes.clone())
    }) as Build<'_>)
        .await
}

/// The write itself: the new file, or the new version. Its item as the service gives it back.
pub async fn write(plan: &Plan, content: &Content) -> Result<Item, Fault> {
    let api = &plan.api;
    let p = plan.provider;
    let mime = if content.mime.is_empty() { mime_of(&plan.name, content.text.is_some()) } else { content.mime.clone() };
    let v = match (p, plan.replace) {
        (Provider::Dropbox, replace) => {
            let path = if replace {
                // the names as the person wrote them (Dropbox matches either way)
                if plan.target.display.is_empty() {
                    plan.target.path.clone()
                } else {
                    plan.target.display.clone()
                }
            } else {
                let folder = if plan.target.id == ROOT_ID { "" } else { plan.target.path.trim_end_matches('/') };
                format!("{folder}/{}", plan.name)
            };
            let mode = if replace { json!({".tag": "update", "update": plan.target.rev}) } else { json!("add") };
            let arg = ascii_json(
                &json!({"path": path, "mode": mode, "autorename": false, "mute": false, "strict_conflict": true}),
            );
            let (url, bytes) = (format!("{}/files/upload", api.ep.content), content.bytes.clone());
            api.json(Box::new(move |http: &reqwest::Client, token: &str| {
                http.post(&url)
                    .bearer_auth(token)
                    .header("Dropbox-API-Arg", arg.clone())
                    .header("content-type", "application/octet-stream")
                    .body(bytes.clone())
            }) as Build<'_>)
                .await?
        }
        (Provider::Google, false) => {
            let metadata = json!({"name": plan.name, "parents": [plan.target.id], "mimeType": mime});
            google_upload(api, reqwest::Method::POST, format!("{}/files", api.ep.content), metadata, content, &mime)
                .await?
        }
        (Provider::Google, true) => {
            // the version before kept for good (Google drops old ones after 30 days or 100 versions): best effort
            if !plan.target.rev.is_empty() {
                let url = format!(
                    "{}/files/{}/revisions/{}",
                    api.ep.api,
                    drives::path_segment(&plan.target.id),
                    drives::path_segment(&plan.target.rev)
                );
                let kept = api
                    .json(Box::new(move |http: &reqwest::Client, token: &str| {
                        http.patch(&url).bearer_auth(token).json(&json!({"keepForever": true}))
                    }) as Build<'_>)
                    .await;
                if let Err(why) = kept {
                    tracing::info!(why = ?why, "drives: Google didn't keep the version before for good");
                }
            }
            let url = format!("{}/files/{}", api.ep.content, drives::path_segment(&plan.target.id));
            google_upload(api, reqwest::Method::PATCH, url, json!({}), content, &mime).await?
        }
        (Provider::Microsoft, false) => {
            let url = format!(
                "{}/me/drive/items/{}:/{}:/content",
                api.ep.api,
                drives::path_segment(&plan.target.id),
                drives::path_segment(&plan.name)
            );
            let (bytes, mime) = (content.bytes.clone(), mime.clone());
            api.json(Box::new(move |http: &reqwest::Client, token: &str| {
                http.put(&url)
                    .bearer_auth(token)
                    .query(&[("@microsoft.graph.conflictBehavior", "fail")])
                    .header("content-type", mime.clone())
                    .body(bytes.clone())
            }) as Build<'_>)
                .await?
        }
        (Provider::Microsoft, true) => {
            let url = format!("{}/me/drive/items/{}/content", api.ep.api, drives::path_segment(&plan.target.id));
            let (bytes, mime, tag) = (content.bytes.clone(), mime.clone(), plan.target.etag.clone());
            api.json(Box::new(move |http: &reqwest::Client, token: &str| {
                let request =
                    http.put(&url).bearer_auth(token).header("content-type", mime.clone()).body(bytes.clone());
                // never over a change made meanwhile (412)
                if tag.is_empty() {
                    request
                } else {
                    request.header("if-match", tag.clone())
                }
            }) as Build<'_>)
                .await?
        }
    };
    let made = drives::item_of(p, &v);
    if made.id.is_empty() {
        return Err(Fault::Unavailable);
    }
    Ok(made)
}

/// « Annuler » on a new file made at once (approval_level.rs `Undo::DriveFile`): put in the service's bin (Google's
/// bin, Dropbox's deleted files, OneDrive's recycle bin: all recoverable there), only while it is the file that was
/// made (`rev`: its revision then). Gone already: nothing to do.
pub async fn undo_file(config: &Config, service: &str, id: &str, rev: &str) -> Result<(), String> {
    let provider = Provider::of(service).ok_or("no such drive")?;
    let api = drives::access(config, provider).await.map_err(|f| f.words(provider))?;
    let now = match api.item_with(id, drives::WRITE_FIELDS).await {
        Ok(item) if item.trashed => return Ok(()),
        Ok(item) => item,
        Err(Fault::NotFound) => return Ok(()),
        Err(fault) => return Err(fault.words(provider)),
    };
    if !rev.is_empty() && now.rev != rev {
        return Err(format!("changed in {} since it was saved: left as it is", provider.label()));
    }
    let url = match provider {
        Provider::Dropbox => format!("{}/files/delete_v2", api.ep.api),
        Provider::Google => format!("{}/files/{}", api.ep.api, drives::path_segment(id)),
        Provider::Microsoft => format!("{}/me/drive/items/{}", api.ep.api, drives::path_segment(id)),
    };
    let (id, rev, tag) = (id.to_string(), now.rev.clone(), now.etag.clone());
    let done = api
        .send(Box::new(move |http: &reqwest::Client, token: &str| match provider {
            Provider::Dropbox => http.post(&url).bearer_auth(token).json(&json!({"path": id, "parent_rev": rev})),
            Provider::Google => http.patch(&url).bearer_auth(token).json(&json!({"trashed": true})),
            Provider::Microsoft => {
                let request = http.delete(&url).bearer_auth(token);
                if tag.is_empty() {
                    request
                } else {
                    request.header("if-match", tag.clone())
                }
            }
        }) as Build<'_>)
        .await;
    match done {
        Ok(_) | Err(Fault::NotFound) => {
            tracing::info!(service = provider.id(), "drives: a new file undone (in the service's bin)");
            Ok(())
        }
        Err(Fault::Changed) => Err(format!("changed in {} since it was saved: left as it is", provider.label())),
        Err(fault) => Err(fault.words(provider)),
    }
}

// -- the call -------------------------------------------------------------------------------------------------------------

fn digest(name: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(name.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    hex::encode(&hash.finalize()[..12])
}

/// The Hermes session of the call under way (its « Validé pour toi » line is grouped by it).
fn call_session() -> Option<String> {
    crate::runs::CALL_SESSION.try_with(|s| s.clone()).ok().flatten().filter(|s| !s.is_empty())
}

/// A call of `agent` (agent_tools.rs, with the state: its card).
pub async fn call(state: &AppState, agent: &AgentConfig, arguments: &Dict) -> Dict {
    let config = state.config();
    if !drives::offered(&config, &agent.name) {
        return tool_failure(
            "Your person shared no folder of their online documents with you.",
            "Tu ne m'as donné accès à aucun dossier de tes documents en ligne : tu peux le faire dans l'app, « Documents en ligne ».",
        );
    }
    let (folder, replace) = (arg(arguments, "folder"), arg(arguments, "replace"));
    let (asked, replacing) = match (folder.is_empty(), replace.is_empty()) {
        (false, true) => (folder, false),
        (true, false) => (replace, true),
        (false, false) => return refused("Give `folder` (a new file) or `replace` (a new version), not both."),
        (true, true) => {
            return refused("Give `folder` and `name` for a new file, or `replace` for a new version of a file.")
        }
    };
    let (text, file) = (arguments.get("text").map(py::py_str).unwrap_or_default(), arg(arguments, "file"));
    let content = {
        let (c, a, f) = (config.clone(), agent.clone(), file.clone());
        let text = text.clone();
        match crate::runs::blocking_with_call_session(move || content_of(&c, &a, &text, &f)).await {
            Ok(Ok(content)) => content,
            Ok(Err(why)) => return refused(&format!("Nothing saved: {why}.")),
            Err(e) => return refused(&format!("Nothing saved: {e}.")),
        }
    };
    let name = if replacing {
        String::new()
    } else {
        let asked_name = arg(arguments, "name");
        let asked_name = if asked_name.is_empty() { content.own_name.clone().unwrap_or_default() } else { asked_name };
        match clean_name(&asked_name) {
            Ok(clean) => name_with_extension(&clean, content.own_name.as_deref()),
            Err(why) => return refused(&format!("Nothing saved: {why}.")),
        }
    };
    let first = match plan(&config, agent, &asked, replacing, &name).await {
        Ok(planned) => planned,
        Err(said) => return said,
    };
    let lang = crate::mail_tidy::language(&config, &agent.name);
    let label = first.provider.label();
    let shown_name = drives::defused(&first.name);
    let kind = if replacing { KIND_REPLACE } else { KIND_CREATE };
    // a new file under « Seulement le critique »: no card, a « Validé pour toi » line with « Annuler »
    let at_once = !replacing && crate::approval_level::auto_for(&config, &agent.name, KIND_CREATE);
    if !at_once {
        let card = crate::person_ok::Card {
            kind,
            tool: TOOL,
            command: format!(
                "{TOOL} {} {} {}",
                drives::reference(first.provider, &first.target.id),
                first.name,
                digest(&first.name, &content.bytes)
            ),
            summary: card_sentence(&lang, replacing, &shown_name, &first.place),
            detail: card_detail(&lang, replacing, label, &content),
            always: None,
        };
        match crate::person_ok::ask(state, agent, card).await {
            crate::person_ok::Answer::Once | crate::person_ok::Answer::Always => {}
            crate::person_ok::Answer::Deny => {
                return refused("Your person said no: nothing was saved. Ask what they want instead.")
            }
            crate::person_ok::Answer::NoAnswer => {
                return refused(
                    "Your person didn't answer the card in time: nothing was saved. Say you'll do it when they agree.",
                )
            }
            crate::person_ok::Answer::Held => {
                return tool_text(
                    "Your person isn't there now: the card waits for them in their app. Nothing is saved until they \
say yes; say so in your report and go on with the rest.",
                    false,
                )
            }
        }
    }
    // checked again: the folder still given, its switch still on, the file unchanged since the card showed it
    let planned = match plan(&config, agent, &asked, replacing, &name).await {
        Ok(planned) => planned,
        Err(said) => {
            tracing::info!(agent = %agent.name, "drives: the write is no longer allowed after the card: nothing written");
            return said;
        }
    };
    if planned.target.id != first.target.id || (replacing && planned.target.rev != first.target.rev) {
        return refused(&format!(
            "« {shown_name} » changed in {label} since your person saw the card: nothing was saved. Read it again first."
        ));
    }
    let made = match write(&planned, &content).await {
        Ok(made) => made,
        Err(fault) => {
            tracing::info!(agent = %agent.name, service = planned.provider.id(), why = ?fault, "drives: write refused");
            if planned.provider == Provider::Google && replacing && matches!(fault, Fault::Refused | Fault::NotFound) {
                // drive.file: a file only inside a picked folder may not be the app's to change (never a wider scope)
                return refused(&format!(
                    "Google lets BipAgents change only the files it made or that your person picked in Google's \
window: they pick « {shown_name} » in the app (Documents en ligne › Google Drive › Ajouter des fichiers), then try \
again. Nothing was saved."
                ));
            }
            return drives::told(planned.provider, &format!("« {shown_name} » not saved"), &fault);
        }
    };
    let reference = drives::reference(planned.provider, &made.id);
    tracing::info!(agent = %agent.name, service = planned.provider.id(), bytes = content.bytes.len(), replace = replacing,
                   "drives: a file written for the agent");
    if replacing {
        return tool_text(
            &format!(
                "New version of « {shown_name} » saved in {} (the old one stays in {label}'s history): {reference}.",
                planned.place
            ),
            false,
        );
    }
    let mut said = format!("Saved: « {shown_name} » in {}, a new file: {reference}.", planned.place);
    if at_once {
        crate::approval_level::record(
            &config,
            crate::approval_level::Line {
                agent: agent.name.clone(),
                kind: KIND_CREATE.into(),
                summary: line_sentence(&lang, &shown_name, &planned.place),
                session_id: call_session(),
                undo: Some(crate::approval_level::Undo::DriveFile {
                    service: planned.provider.id().into(),
                    id: made.id.clone(),
                    rev: made.rev.clone(),
                }),
                ..Default::default()
            },
        );
        said.push_str(
            " Done at once (your person lets new files through without asking): tell them in a few words; the app \
shows it with « Annuler ».",
        );
    }
    tool_text(&said, false)
}

#[cfg(test)]
#[path = "drives_write_tests.rs"]
mod tests;
