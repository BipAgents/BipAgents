//! A payment page's text as the person sees it (fix-pay-total-check, Correction 7): read from what Chrome RENDERED
//! (`DOMSnapshot.captureSnapshot`: the layout's text boxes with their positions, the computed styles asked here, input
//! values, shadow roots, closed ones included, and the same-process frames' documents), never rebuilt from the DOM's
//! text pieces. Six reviews found layouts where the old reader (a page script joining text nodes by their block,
//! card_fill.js `pageText`) read another amount than the one drawn: cents in their own flex item (« 32 » « ,05 € »),
//! « 156<sup>€90</sup> », a total in a read-only field, a total in a closed web component, in the shop's own srcdoc
//! frame… Here the text boxes are joined into visual lines by geometry:
//! - two boxes are on the same line when they overlap vertically by half the smaller one's height (a raised or lowered
//!   piece, cents by flex alignment or `<sup>`, stays on its line);
//! - on a line, boxes that touch are glued (« 32 » + « ,05 € » = « 32,05 € », « 156 » + « €90 » = « 156€90 »), two
//!   raised digits right after digits are cents (« 12 » + raised « 50 » = « 12,50 »), any other gap is a space;
//! - a new line starts at a box that is not on the current line (the next row, or a column read after another).
//!
//! What is laid out but not really visible (off the page's box, clipped by an ancestor, transparent, tiny, visibility
//! hidden, aria-hidden) goes to `all` only: it can block « approved », never support it (page_total.rs). Text inside a
//! form control (INPUT, TEXTAREA, SELECT: the card's fields among them) is never read; the value of a read-only or
//! disabled field that is not a card field is read as « [champ] » (all only). Struck-out text is marked « [barré] », the
//! amounts of the smallest box holding an add button (one price, no total's words) « [suggestion] », text in a closed
//! shadow root goes to `closed` too (any amount above the approved one there blocks). Past `MAX_CHARS`, `truncated`.
//!
//! The snapshot holds every input's live value, the card's fields' too once typed: card_fill.rs wipes it (`wipe`) as
//! soon as this module has read it; nothing of it is logged.

use std::collections::HashMap;

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// The computed styles asked of the snapshot, in this order (`STYLE_*`).
pub const STYLES: [&str; 8] =
    ["visibility", "opacity", "font-size", "color", "text-decoration-line", "clip-path", "overflow-x", "overflow-y"];
const VISIBILITY: usize = 0;
const OPACITY: usize = 1;
const FONT_SIZE: usize = 2;
const COLOR: usize = 3;
const DECORATION: usize = 4;
const CLIP_PATH: usize = 5;
const OVERFLOW_X: usize = 6;
const OVERFLOW_Y: usize = 7;

/// The most characters read per document; past them `truncated` (never « approved »: no silent cap).
pub const MAX_CHARS: usize = 200_000;

/// One document of the snapshot, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocText {
    pub frame: String,
    pub url: String,
    /// What is really visible, in visual lines.
    pub shown: String,
    /// Everything laid out (read-only fields' values included), in visual lines.
    pub all: String,
    /// The text of closed shadow roots, in visual lines (empty: none).
    pub closed: String,
    pub truncated: bool,
}

/// An add button's words (FR/EN/DE/ES/NL), its whole text.
static ADD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^[\s+]*(ajouter( au panier)?|add( to (cart|bag|basket))?|in den warenkorb|hinzuf[üu]gen|a[ñn]adir( al carrito)?|toevoegen( aan (winkelwagen|winkelmand))?|in (de )?winkel(wagen|mand))[\s!]*$",
    )
    .expect("add pattern")
});
static TOTAL_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)total|[àa] payer|montant|amount|betrag|gesamt|summe|importe|totaal|bedrag|te betalen|zu zahlen|a pagar")
        .expect("total words pattern")
});
static PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\d[\d .,'\x{a0}\x{202f}]*[.,]\d{2}|\d+\s?(€|eur|\$|£|chf)").expect("price pattern")
});

/// A text box placed on the page.
#[derive(Debug, Clone)]
struct Piece {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    text: String,
    shown: bool,
    closed: bool,
    /// The box's own text began / ended with a space (trimmed here): never glued on that side.
    lead: bool,
    trail: bool,
}

/// The snapshot's arrays of one document, with the strings table.
struct Doc<'a> {
    strings: &'a [Value],
    doc: &'a Value,
    parent: Vec<i64>,
    node_name: Vec<String>,
    /// node → its first layout entry.
    layout_of: HashMap<usize, usize>,
    width: f64,
    height: f64,
}

fn ints(v: Option<&Value>) -> Vec<i64> {
    v.and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_i64().unwrap_or(-1)).collect()).unwrap_or_default()
}

/// A rare-data table (`{index: [...], value: [...]}`) as node → value index.
fn rare(v: Option<&Value>) -> HashMap<usize, i64> {
    let index = ints(v.and_then(|r| r.get("index")));
    let value = ints(v.and_then(|r| r.get("value")));
    index.into_iter().zip(value).filter(|(i, _)| *i >= 0).map(|(i, v)| (i as usize, v)).collect()
}

impl<'a> Doc<'a> {
    fn string(&self, index: i64) -> &str {
        if index < 0 {
            return "";
        }
        self.strings.get(index as usize).and_then(Value::as_str).unwrap_or("")
    }

    fn style(&self, layout: usize, which: usize) -> &str {
        let index = self.doc["layout"]["styles"].get(layout).and_then(|s| s.get(which)).and_then(Value::as_i64);
        index.map_or("", |i| self.string(i))
    }

    fn bounds(&self, key: &str, i: usize) -> Option<(f64, f64, f64, f64)> {
        let b = self.doc[key]["bounds"].get(i)?.as_array()?;
        let f = |k: usize| b.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        Some((f(0), f(1), f(2), f(3)))
    }

    fn attributes(&self, node: usize) -> Vec<(String, String)> {
        let list = self.doc["nodes"]["attributes"].get(node).and_then(Value::as_array).cloned().unwrap_or_default();
        list.chunks(2)
            .map(|p| {
                let s = |v: Option<&Value>| self.string(v.and_then(Value::as_i64).unwrap_or(-1)).to_string();
                (s(p.first()), s(p.get(1)))
            })
            .collect()
    }

    fn ancestors(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        let mut at = node as i64;
        std::iter::from_fn(move || {
            at = *self.parent.get(at as usize)?;
            (at >= 0).then_some(at as usize)
        })
    }
}

/// The documents of a `DOMSnapshot.captureSnapshot` answer, read (its `strings` and `documents`).
pub fn read_snapshot(snapshot: &Value) -> Vec<DocText> {
    let strings = snapshot.get("strings").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let docs = snapshot.get("documents").and_then(Value::as_array).cloned().unwrap_or_default();
    docs.iter().map(|doc| read_document(strings, doc)).collect()
}

fn read_document(strings: &[Value], doc: &Value) -> DocText {
    let nodes = &doc["nodes"];
    let parent = ints(nodes.get("parentIndex"));
    let names = ints(nodes.get("nodeName"));
    let string = |i: i64| if i < 0 { "" } else { strings.get(i as usize).and_then(Value::as_str).unwrap_or("") };
    let node_name: Vec<String> = names.iter().map(|i| string(*i).to_uppercase()).collect();
    let layout_nodes = ints(doc["layout"].get("nodeIndex"));
    let mut layout_of = HashMap::new();
    for (li, ni) in layout_nodes.iter().enumerate() {
        if *ni >= 0 {
            layout_of.entry(*ni as usize).or_insert(li);
        }
    }
    let d = Doc {
        strings,
        doc,
        parent,
        node_name,
        layout_of,
        width: doc.get("contentWidth").and_then(Value::as_f64).unwrap_or(f64::MAX),
        height: doc.get("contentHeight").and_then(Value::as_f64).unwrap_or(f64::MAX),
    };
    let shadow = rare(nodes.get("shadowRootType"));
    let input_value = rare(nodes.get("inputValue"));
    let opacities = doc["layout"].get("textColorOpacities").and_then(Value::as_array).cloned().unwrap_or_default();

    // per node, memoised: inside a form control or a user-agent root (never read), inside a closed root, hidden by an
    // ancestor (aria-hidden, clip-path), struck out, a suggestion box
    let count = d.node_name.len();
    let control = |n: usize| matches!(d.node_name[n].as_str(), "INPUT" | "TEXTAREA" | "SELECT" | "OPTION" | "SCRIPT" | "STYLE" | "NOSCRIPT" | "TEMPLATE");
    let mut out_of_reach = vec![false; count];
    let mut in_closed = vec![false; count];
    for n in 0..count {
        let mut skip = control(n);
        let mut closed = false;
        for a in d.ancestors(n) {
            if control(a) || shadow.get(&a).is_some_and(|v| d.string(*v) == "user-agent") {
                skip = true;
            }
            if shadow.get(&a).is_some_and(|v| d.string(*v) == "closed") {
                closed = true;
            }
        }
        out_of_reach[n] = skip;
        in_closed[n] = closed;
    }
    let element = |n: usize| d.ancestors(n).next().filter(|_| d.node_name[n] == "#TEXT").unwrap_or(n);
    let hidden_by_ancestor = |n: usize, (x, y, w, h): (f64, f64, f64, f64)| -> bool {
        let mut opacity = 1.0;
        for a in std::iter::once(n).chain(d.ancestors(n)) {
            if d.attributes(a).iter().any(|(k, v)| k == "aria-hidden" && v == "true") {
                return true;
            }
            let Some(&li) = d.layout_of.get(&a) else { continue };
            opacity *= d.style(li, OPACITY).parse::<f64>().unwrap_or(1.0);
            let clip = d.style(li, CLIP_PATH);
            if !clip.is_empty() && clip != "none" {
                return true;
            }
            let name = d.node_name[a].as_str();
            let overflow = format!("{} {}", d.style(li, OVERFLOW_X), d.style(li, OVERFLOW_Y));
            if a != n && name != "BODY" && name != "HTML" && ["hidden", "clip", "scroll", "auto"].iter().any(|o| overflow.contains(o)) {
                if let Some((bx, by, bw, bh)) = d.bounds("layout", li) {
                    if bw < 1.0 || bh < 1.0 || x + w <= bx || x >= bx + bw || y + h <= by || y >= by + bh {
                        return true;
                    }
                }
            }
        }
        opacity < 0.1
    };
    let struck = |n: usize| -> bool {
        for (level, a) in std::iter::once(n).chain(d.ancestors(n)).enumerate() {
            if matches!(d.node_name[a].as_str(), "DEL" | "S" | "STRIKE") {
                return true;
            }
            if level < 4 {
                if let Some(&li) = d.layout_of.get(&a) {
                    if d.style(li, DECORATION).contains("line-through") {
                        return true;
                    }
                }
            }
        }
        false
    };
    // children, for the suggestion boxes' own text and buttons
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (n, p) in d.parent.iter().enumerate() {
        if *p >= 0 && (*p as usize) < count {
            children[*p as usize].push(n);
        }
    }
    let values = ints(nodes.get("nodeValue"));
    let subtree_text = |root: usize, limit: usize| -> String {
        let mut out = String::new();
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if out.len() > limit {
                break;
            }
            if d.node_name[n] == "#TEXT" {
                out.push_str(d.string(*values.get(n).unwrap_or(&-1)));
                out.push(' ');
            }
            stack.extend(children[n].iter().rev());
        }
        out
    };
    let mut boxes: HashMap<usize, Option<bool>> = HashMap::new();
    let mut suggestion = |n: usize| -> bool {
        for (level, a) in std::iter::once(n).chain(d.ancestors(n)).enumerate() {
            if level >= 6 || matches!(d.node_name[a].as_str(), "BODY" | "HTML") {
                break;
            }
            let decided = *boxes.entry(a).or_insert_with(|| {
                let words = subtree_text(a, 3000);
                if words.len() > 3000 {
                    return None;
                }
                let mut stack = vec![a];
                let mut button = false;
                while let Some(m) = stack.pop() {
                    let name = d.node_name[m].as_str();
                    let attrs = d.attributes(m);
                    let role = attrs.iter().any(|(k, v)| k == "role" && v == "button");
                    let input_button = name == "INPUT"
                        && attrs.iter().any(|(k, v)| k == "type" && matches!(v.as_str(), "button" | "submit"));
                    if name == "BUTTON" || name == "A" || role || input_button {
                        let own = if input_button {
                            attrs.iter().find(|(k, _)| k == "value").map(|(_, v)| v.clone()).unwrap_or_default()
                        } else {
                            subtree_text(m, 200)
                        };
                        if ADD.is_match(own.trim()) {
                            button = true;
                            break;
                        }
                    }
                    stack.extend(children[m].iter());
                }
                button.then(|| PRICE.find_iter(&words).count() <= 1 && !TOTAL_WORDS.is_match(&words))
            });
            if let Some(is) = decided {
                return is; // the smallest box with an add button decides
            }
        }
        false
    };

    // the text boxes, in layout order
    let layout_text = ints(doc["layout"].get("text"));
    let tb_layout = ints(doc["textBoxes"].get("layoutIndex"));
    let tb_start = ints(doc["textBoxes"].get("start"));
    let tb_length = ints(doc["textBoxes"].get("length"));
    let mut pieces: Vec<Piece> = Vec::new();
    let mut utf16_cache: HashMap<usize, Vec<u16>> = HashMap::new();
    let mut chars = 0usize;
    let mut truncated = false;
    for (k, li) in tb_layout.iter().enumerate() {
        let Some(li) = usize::try_from(*li).ok() else { continue };
        let Some(&ni) = layout_nodes.get(li) else { continue };
        let Ok(ni) = usize::try_from(ni) else { continue };
        if ni >= count || out_of_reach[ni] {
            continue;
        }
        let full = utf16_cache.entry(li).or_insert_with(|| d.string(*layout_text.get(li).unwrap_or(&-1)).encode_utf16().collect());
        let (start, length) = (tb_start.get(k).copied().unwrap_or(0).max(0) as usize, tb_length.get(k).copied().unwrap_or(0).max(0) as usize);
        let Some(slice) = full.get(start..(start + length).min(full.len())) else { continue };
        let raw = String::from_utf16_lossy(slice);
        let (lead, trail) = (raw.starts_with(char::is_whitespace), raw.ends_with(char::is_whitespace));
        let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        chars += text.len() + 1;
        if chars > MAX_CHARS {
            truncated = true;
            break;
        }
        let Some(bounds @ (x, y, w, h)) = d.bounds("textBoxes", k) else { continue };
        let owner = element(ni);
        let style_li = d.layout_of.get(&owner).copied().unwrap_or(li);
        let visibility = d.style(style_li, VISIBILITY);
        let font: f64 = d.style(style_li, FONT_SIZE).trim_end_matches("px").parse().unwrap_or(16.0);
        let alpha = opacities.get(li).and_then(Value::as_f64).unwrap_or_else(|| color_alpha(d.style(style_li, COLOR)));
        let in_page = w >= 1.0 && h >= 1.0 && x + w > 0.0 && y + h > 0.0 && x < d.width && y < d.height;
        let shown = in_page
            && visibility != "hidden"
            && visibility != "collapse"
            && font >= 6.0
            && alpha >= 0.2
            && !hidden_by_ancestor(owner, bounds);
        let mark = if text.chars().any(|c| c.is_ascii_digit()) {
            if struck(owner) {
                "[barré] "
            } else if suggestion(owner) {
                "[suggestion] "
            } else {
                ""
            }
        } else {
            ""
        };
        pieces.push(Piece { x, y, w, h, text: format!("{mark}{text}"), shown, closed: in_closed[ni], lead, trail });
    }
    // read-only / disabled fields that are not card fields: their value, laid out, never shown
    let mut fields: Vec<Piece> = Vec::new();
    for (n, vi) in &input_value {
        let n = *n;
        if n >= count || d.node_name[n] != "INPUT" {
            continue;
        }
        let attrs = d.attributes(n);
        let has = |k: &str| attrs.iter().any(|(a, _)| a == k);
        let attr = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.to_lowercase()).unwrap_or_default();
        let kind = attr("type");
        if !(has("readonly") || has("disabled"))
            || attr("autocomplete").contains("cc-")
            || ["hidden", "password", "checkbox", "radio", "submit", "button", "image", "file"].contains(&kind.as_str())
        {
            continue;
        }
        let value = d.string(*vi).split_whitespace().collect::<Vec<_>>().join(" ");
        let Some(&li) = d.layout_of.get(&n) else { continue };
        let Some((x, y, w, h)) = d.bounds("layout", li) else { continue };
        if value.chars().any(|c| c.is_ascii_digit()) && w >= 1.0 && h >= 1.0 {
            fields.push(Piece { x, y, w, h, text: format!("[champ] {value}"), shown: false, closed: false, lead: true, trail: true });
        }
    }
    // a folded <details> (a mobile checkout's « Récapitulatif »): its text has no layout, but it is the page's (the
    // person opens it with one tap) — read as laid-out text, one line per block (Relecture 6, f6)
    let mut folded = Vec::new();
    for n in 0..count {
        let open = d.attributes(n).iter().any(|(k, _)| k == "open");
        if d.node_name[n] != "DETAILS" || open || out_of_reach[n] {
            continue;
        }
        let mut text = String::new();
        folded_text(&d, &children, &values, n, &mut text);
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            folded.push(Piece {
                x: 0.0,
                y: f64::MAX / 4.0 + folded.len() as f64 * 100.0,
                w: 1.0,
                h: 10.0,
                text: line.split_whitespace().collect::<Vec<_>>().join(" "),
                shown: false,
                closed: in_closed[n],
                lead: true,
                trail: true,
            });
        }
    }
    let all = lines(pieces.iter().chain(fields.iter()).chain(folded.iter()));
    let shown = lines(pieces.iter().filter(|p| p.shown));
    let closed = lines(pieces.iter().filter(|p| p.closed));
    DocText {
        frame: d.string(doc.get("frameId").and_then(Value::as_i64).unwrap_or(-1)).to_string(),
        url: d.string(doc.get("documentURL").and_then(Value::as_i64).unwrap_or(-1)).to_string(),
        shown,
        all,
        closed,
        truncated,
    }
}

/// The text of a folded <details> below `n` that has no layout (its summary aside, already drawn): a new line at each
/// block element.
fn folded_text(d: &Doc<'_>, children: &[Vec<usize>], values: &[i64], n: usize, out: &mut String) {
    let name = d.node_name[n].as_str();
    if matches!(name, "SUMMARY" | "SCRIPT" | "STYLE" | "TEMPLATE" | "INPUT" | "TEXTAREA" | "SELECT") || out.len() > MAX_CHARS {
        return;
    }
    if name == "#TEXT" {
        if !d.layout_of.contains_key(&n) {
            out.push_str(d.string(*values.get(n).unwrap_or(&-1)));
        }
        return;
    }
    let block = matches!(name, "P" | "DIV" | "LI" | "TR" | "DT" | "DD" | "H1" | "H2" | "H3" | "H4" | "H5" | "H6" | "SECTION" | "TABLE" | "UL" | "OL" | "BR");
    if block {
        out.push('\n');
    }
    for c in &children[n] {
        folded_text(d, children, values, *c, out);
    }
    if block {
        out.push('\n');
    }
}

/// A CSS colour's alpha (« rgba(0, 0, 0, 0) » → 0); 1 when it says none.
fn color_alpha(color: &str) -> f64 {
    let numbers: Vec<f64> = color
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if color.starts_with("rgba") && numbers.len() == 4 {
        numbers[3]
    } else {
        1.0
    }
}

/// Pieces joined into visual lines (the rule: this module's head).
fn lines<'a>(pieces: impl Iterator<Item = &'a Piece>) -> String {
    let mut out = String::new();
    let mut line: Option<(f64, f64)> = None; // the current line's top and bottom
    let mut last: Option<&Piece> = None;
    for p in pieces {
        let (top, bottom) = (p.y, p.y + p.h);
        let same_line = match (line, last) {
            (Some((lt, lb)), Some(_)) => {
                let overlap = bottom.min(lb) - top.max(lt);
                overlap >= 0.5 * p.h.min(lb - lt).max(1.0)
            }
            _ => false,
        };
        if same_line {
            let prev = last.expect("a piece before on its line");
            let gap = p.x - (prev.x + prev.w);
            let glued = (gap.abs() < 1.0 || (gap < 0.0 && gap > -prev.w)) && !prev.trail && !p.lead;
            let raised = p.y + p.h < prev.y + prev.h - 1.0 && p.h < prev.h * 0.95;
            let cents = prev.text.ends_with(|c: char| c.is_ascii_digit())
                && p.text.len() >= 2
                && p.text.chars().take(2).all(|c| c.is_ascii_digit())
                && p.text.chars().nth(2).is_none_or(|c| !c.is_ascii_digit())
                && raised;
            if cents && glued {
                out.push(',');
            } else if !glued {
                out.push(' ');
            }
            out.push_str(&p.text);
            let (lt, lb) = line.expect("a line");
            line = Some((lt.min(top), lb.max(bottom)));
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&p.text);
            line = Some((top, bottom));
        }
        last = Some(p);
    }
    out
}

/// Every string of a snapshot overwritten (its input values: the card's fields once typed), as soon as it was read.
pub fn wipe(value: &mut Value) {
    use zeroize::Zeroize;
    match value {
        Value::String(s) => s.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe),
        Value::Object(map) => map.values_mut().for_each(wipe),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A snapshot of one document: `texts` are (text, x, y, w, h, parent element's name, its attributes, styles).
    type Layout<'a> = Option<(&'a [(usize, &'a str)], (f64, f64, f64, f64))>;

    struct Build {
        strings: Vec<String>,
        parent: Vec<i64>,
        names: Vec<i64>,
        values: Vec<i64>,
        attrs: Vec<Vec<i64>>,
        layout_nodes: Vec<i64>,
        layout_styles: Vec<Vec<i64>>,
        layout_bounds: Vec<Vec<f64>>,
        layout_text: Vec<i64>,
        boxes: (Vec<i64>, Vec<Vec<f64>>, Vec<i64>, Vec<i64>),
        shadow: (Vec<i64>, Vec<i64>),
        input: (Vec<i64>, Vec<i64>),
    }

    impl Build {
        fn new() -> Build {
            let mut b = Build {
                strings: Vec::new(),
                parent: Vec::new(),
                names: Vec::new(),
                values: Vec::new(),
                attrs: Vec::new(),
                layout_nodes: Vec::new(),
                layout_styles: Vec::new(),
                layout_bounds: Vec::new(),
                layout_text: Vec::new(),
                boxes: (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
                shadow: (Vec::new(), Vec::new()),
                input: (Vec::new(), Vec::new()),
            };
            b.node(-1, "#document", &[], None); // 0
            b.node(0, "BODY", &[], Some((&[], (0.0, 0.0, 800.0, 2000.0)))); // 1
            b
        }

        fn s(&mut self, text: &str) -> i64 {
            if let Some(i) = self.strings.iter().position(|s| s == text) {
                return i as i64;
            }
            self.strings.push(text.to_string());
            (self.strings.len() - 1) as i64
        }

        fn node(&mut self, parent: i64, name: &str, attrs: &[(&str, &str)], layout: Layout<'_>) -> i64 {
            let n = self.parent.len() as i64;
            self.parent.push(parent);
            let name = self.s(name);
            self.names.push(name);
            self.values.push(-1);
            let a: Vec<i64> = attrs.iter().flat_map(|(k, v)| [*k, *v]).map(|x| self.s(x)).collect();
            self.attrs.push(a);
            if let Some((styles, (x, y, w, h))) = layout {
                let mut st = vec![-1; STYLES.len()];
                for (i, v) in styles {
                    st[*i] = self.s(v);
                }
                self.layout_nodes.push(n);
                self.layout_styles.push(st);
                self.layout_bounds.push(vec![x, y, w, h]);
                self.layout_text.push(-1);
            }
            n
        }

        /// An element holding one text drawn at (x, y, w, h).
        fn text(&mut self, parent: i64, tag: &str, text: &str, (x, y, w, h): (f64, f64, f64, f64), styles: &[(usize, &str)]) -> i64 {
            let el = self.node(parent, tag, &[], Some((styles, (x, y, w, h))));
            let t = self.node(el, "#text", &[], None);
            self.values[t as usize] = self.s(text);
            self.layout_nodes.push(t);
            self.layout_styles.push(vec![-1; STYLES.len()]);
            self.layout_bounds.push(vec![x, y, w, h]);
            let ti = self.s(text);
            self.layout_text.push(ti);
            let li = (self.layout_nodes.len() - 1) as i64;
            self.boxes.0.push(li);
            self.boxes.1.push(vec![x, y, w, h]);
            self.boxes.2.push(0);
            self.boxes.3.push(text.encode_utf16().count() as i64);
            el
        }

        fn snapshot(&self) -> Value {
            json!({"strings": self.strings, "documents": [{
                "documentURL": -1, "frameId": -1, "contentWidth": 800.0, "contentHeight": 2000.0,
                "nodes": {"parentIndex": self.parent, "nodeName": self.names, "nodeValue": self.values,
                          "attributes": self.attrs,
                          "shadowRootType": {"index": self.shadow.0, "value": self.shadow.1},
                          "inputValue": {"index": self.input.0, "value": self.input.1}},
                "layout": {"nodeIndex": self.layout_nodes, "styles": self.layout_styles, "bounds": self.layout_bounds,
                           "text": self.layout_text},
                "textBoxes": {"layoutIndex": self.boxes.0, "bounds": self.boxes.1, "start": self.boxes.2,
                              "length": self.boxes.3}}]})
        }
    }

    fn read(b: &Build) -> DocText {
        read_snapshot(&b.snapshot()).remove(0)
    }

    #[test]
    fn pieces_are_joined_as_they_are_drawn() {
        let mut b = Build::new();
        b.text(1, "SPAN", "Montant de votre panier : 27,97 €", (8.0, 64.0, 271.0, 19.0), &[]);
        b.text(1, "SPAN", "Total à payer ", (8.0, 114.0, 108.0, 19.0), &[]);
        // cents by flex alignment: « 32 » big, « ,05 € » touching it, on its top
        b.text(1, "SPAN", "32", (116.0, 99.0, 41.0, 38.0), &[]);
        b.text(1, "SPAN", ",05 €", (157.0, 99.0, 41.0, 19.0), &[]);
        // « 156<sup>€90</sup> »
        b.text(1, "SPAN", "Prix", (8.0, 156.0, 35.0, 19.0), &[]);
        b.text(1, "SPAN", "156", (47.0, 156.0, 31.0, 19.0), &[]);
        b.text(1, "SUP", "€90", (78.0, 153.0, 25.0, 15.0), &[]);
        // « Payer 12<sup>50</sup> € »
        b.text(1, "SPAN", "Payer 12", (8.0, 200.0, 70.0, 19.0), &[]);
        b.text(1, "SUP", "50", (78.0, 197.0, 14.0, 12.0), &[]);
        b.text(1, "SPAN", " €", (92.0, 200.0, 14.0, 19.0), &[]);
        let got = read(&b);
        assert_eq!(
            got.shown,
            "Montant de votre panier : 27,97 €\nTotal à payer 32,05 €\nPrix 156€90\nPayer 12,50 €"
        );
        assert_eq!(got.all, got.shown);
    }

    #[test]
    fn hidden_pieces_are_laid_out_only_and_form_controls_never_read() {
        let mut b = Build::new();
        b.text(1, "P", "Total à payer 27,97 €", (8.0, 10.0, 200.0, 19.0), &[]);
        b.text(1, "P", "Total à payer 35,00 €", (-10000.0, 40.0, 200.0, 19.0), &[]);
        b.text(1, "P", "Total 50,00 €", (8.0, 70.0, 200.0, 19.0), &[(VISIBILITY, "hidden")]);
        b.text(1, "P", "Total 60,00 €", (8.0, 100.0, 200.0, 19.0), &[(FONT_SIZE, "1px")]);
        b.text(1, "P", "Total 70,00 €", (8.0, 130.0, 200.0, 19.0), &[(COLOR, "rgba(0, 0, 0, 0)")]);
        // a card field's own text (its user-agent root) and a read-only field
        let input = b.node(1, "INPUT", &[("autocomplete", "cc-number")], Some((&[], (8.0, 160.0, 200.0, 19.0))));
        b.text(input, "DIV", "4242 4242 4242 4242", (8.0, 160.0, 200.0, 19.0), &[]);
        let ro = b.node(1, "INPUT", &[("readonly", ""), ("type", "text")], Some((&[], (8.0, 190.0, 100.0, 19.0))));
        let v = b.s("32,87 €");
        b.input = (vec![ro], vec![v]);
        let got = read(&b);
        assert_eq!(got.shown, "Total à payer 27,97 €");
        assert!(got.all.contains("Total à payer 35,00 €") && got.all.contains("Total 50,00 €"), "{}", got.all);
        assert!(got.all.contains("Total 60,00 €") && got.all.contains("Total 70,00 €"), "{}", got.all);
        assert!(got.all.contains("[champ] 32,87 €"), "{}", got.all);
        assert!(!got.all.contains("4242"), "{}", got.all);
    }

    #[test]
    fn closed_roots_struck_prices_and_suggestions_are_marked() {
        let mut b = Build::new();
        let host = b.node(1, "DIV", &[], Some((&[], (8.0, 10.0, 300.0, 19.0))));
        let root = b.node(host, "#document-fragment", &[], None);
        let c = b.s("closed");
        b.shadow = (vec![root], vec![c]);
        b.text(1, "SPAN", "Total à payer", (8.0, 10.0, 100.0, 19.0), &[]);
        b.text(root, "B", "32,87 €", (110.0, 10.0, 60.0, 19.0), &[]);
        b.text(1, "DEL", "35,00 €", (8.0, 40.0, 60.0, 19.0), &[]);
        let card = b.node(1, "DIV", &[], Some((&[], (8.0, 70.0, 300.0, 40.0))));
        b.text(card, "P", "Coffret soin 39,90 €", (8.0, 70.0, 150.0, 19.0), &[]);
        let button = b.text(card, "BUTTON", "+ Ajouter", (160.0, 70.0, 80.0, 19.0), &[]);
        let _ = button;
        let got = read(&b);
        assert!(got.shown.starts_with("Total à payer 32,87 €"), "{}", got.shown);
        assert_eq!(got.closed, "32,87 €");
        assert!(got.shown.contains("[barré] 35,00 €") && got.shown.contains("[suggestion] Coffret soin 39,90 €"), "{}", got.shown);
    }

    #[test]
    fn a_folded_details_is_read_as_laid_out_text() {
        let mut b = Build::new();
        b.text(1, "P", "Montant de votre panier : 27,97 €", (8.0, 10.0, 260.0, 19.0), &[]);
        let details = b.node(1, "DETAILS", &[], Some((&[], (8.0, 40.0, 300.0, 19.0))));
        b.text(details, "SUMMARY", "Récapitulatif", (8.0, 40.0, 100.0, 19.0), &[]);
        let p = b.node(details, "P", &[], None);
        let bold = b.node(p, "B", &[], None);
        let t = b.node(bold, "#text", &[], None);
        let v = b.s("Total à payer 32,87 €");
        b.values[t as usize] = v;
        let got = read(&b);
        assert!(got.all.ends_with("Total à payer 32,87 €"), "{}", got.all);
        assert!(!got.shown.contains("32,87"), "{}", got.shown);
    }

    #[test]
    fn past_the_cap_the_read_says_it_is_cut() {
        let mut b = Build::new();
        let long = "x".repeat(1000);
        for i in 0..250 {
            b.text(1, "P", &long, (8.0, 10.0 + 20.0 * i as f64, 700.0, 19.0), &[]);
        }
        assert!(read(&b).truncated);
    }

    #[test]
    fn a_snapshots_strings_are_wiped() {
        let mut v = json!({"strings": ["4242424242424242", "123"], "n": [1, {"s": "x"}]});
        wipe(&mut v);
        assert_eq!(v, json!({"strings": ["", ""], "n": [1, {"s": ""}]}));
    }
}
