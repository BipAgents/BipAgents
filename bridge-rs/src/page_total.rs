//! The total a payment page shows, read by the bridge itself (fix-pay-total-check, 19/11): before the card is typed,
//! and again right before the pay button is pressed (card_pay.rs), the bridge reads the text of the page and of its
//! card frames (card_fill.rs `Page::page_texts`: never a field's value, no digit run that could be a card number) and
//! compares the total with the amount the person approved, so that the agent never has to compare numbers itself
//! (Vie on a CAWL page, 19/11: « Total à payer: EUR 27.97 », 27.97 approved, and the agent cancelled).
//!
//! The rule (Relecture 1, 19/11): the reading may stop a payment freely, but it says « the total is the approved
//! amount » only when that is beyond doubt. Any doubt is `Unread`: the agent is told as before, never a confident
//! « approved ».
//!
//! How the amounts are read, in each text:
//! - every number next to a currency mark (cards.rs `mark_near`: « € », « EUR », « euros », « $ »…, before or after it,
//!   spaces of any width between) read as shops write it (cards.rs `amount_of`: « 27,97 », « 27.97 », « 1 234,56 »,
//!   « 1.234,56 », « 1,234.56 »), « 27€97 » and « 27 ,97 € » too; a number glued to a quantity (« Qté 2 27,97 € ») is
//!   read from its last group; a digit run of 12 or more (a card number the page shows back) is never read;
//! - an instalment (« 3 x 9,32 € », « 3 fois 9,32 € », « 9,32 € / mois ») is left out;
//! - each amount gets a class from its label (the words on its line before it, after any amount before it, brackets
//!   left out; else the line above when it holds no amount): a total to pay (« Total à payer », « Amount due »,
//!   « Zu zahlen »…), a total (« Total », « Montant », « Betrag »…), a part (« Sous-total », « Livraison », « HT »,
//!   « TVA », « Total articles », anything in brackets: « (dont 4,66 € de TVA) »), a mention that is never charged
//!   (« dès 49 € », « jusqu'à », « maximum », « au lieu de »), or none.
//!
//! Each text comes twice: what is really visible (in the page box, not clipped, not transparent, not tiny, not
//! aria-hidden) and everything laid out (off-screen text included). Then, with the payment's currency:
//! 1. a visible total or total to pay above the approved amount: `Other` (the page asks more), the payment
//!    provider's own frame or page first, a total to pay before a total;
//! 2. any laid-out amount above the approved one that is not a mention, or any amount in another currency that is not
//!    a mention (DCC): `Unread`;
//! 3. a provider's frame or page that shows amounts must show the approved one as its largest, and every visible total
//!    to pay must be the approved one; then the approved amount labelled as a total (or, with no total labelled at
//!    all, the largest visible amount): `Approved`;
//! 4. the approved amount nowhere on the page and one clear visible total (below it): `Other`;
//! 5. else `Unread`.

use std::sync::LazyLock;

use regex::Regex;

use crate::cards::{amount_of, mark_near};

/// One frame's text as card_fill.rs `pageText` reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageText {
    /// What is really visible.
    pub shown: String,
    /// Everything laid out (off-screen, clipped, transparent or tiny text included; never `display: none`).
    pub all: String,
    /// The payment provider's own frame or page (CAWL's page, Stripe's frame…), not the shop's.
    pub provider: bool,
    /// The frame's text was longer than what is read (`pageText`'s cap): never « approved » (Relecture 2, 3).
    pub truncated: bool,
    /// The text of closed shadow roots (read through DevTools, apart from its labels): any amount above the approved one
    /// in it blocks « approved » (Relecture 5, e3).
    pub closed: bool,
}

impl PageText {
    /// A text read as wholly visible, of the shop's page.
    pub fn plain(text: &str) -> PageText {
        PageText { shown: text.to_string(), all: text.to_string(), provider: false, truncated: false, closed: false }
    }
}

/// What the page says of the payment's total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTotal {
    /// The page's total is the amount the person approved, beyond doubt.
    Approved,
    /// The page's total, in the payment's currency, is another amount (cents).
    Other(i64),
    /// The bridge saw something that keeps it from paying (Relecture 2, 1): stopped, never left to the agent.
    Doubt(Doubt),
    /// No total the bridge can read for sure: what it saw, for the log.
    Unread(Seen),
}

/// What the bridge saw that stops a payment without a clear total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Doubt {
    /// An amount above the approved one (cents) on the page, not labelled as its total.
    Higher(i64),
    /// An amount in another currency (cents, code): a conversion the page offers (DCC).
    Foreign(i64, &'static str),
    /// The page's amounts disagree with the approved one in a way it can't name (a « to pay » amount below it, the
    /// provider's own amount another one, a total without its currency on a page that shows none).
    Unclear,
}

/// What was seen on a page whose total could not be read (logged, never the text: no card data, no address).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// Laid-out amounts in the payment's currency (cents, in order), and how many were labelled as a total.
    pub amounts: Vec<i64>,
    pub labelled: usize,
    /// Amounts in another currency.
    pub other_currency: usize,
}

/// One amount of a page's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    pub cents: i64,
    pub codes: &'static [&'static str],
    pub class: Class,
    /// The value of a read-only field (`pageText` marks it « [champ] »): any one above the approved amount blocks
    /// « approved » unless its own words make it a part or a mention (« Remise 99,99 € ») (Relecture 6, f2).
    pub field: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Never charged: a threshold, a limit, an old price (« dès 49 € », « maximum », « au lieu de »).
    Mention,
    /// A part of the total (« Sous-total », « Livraison », « TVA », in brackets).
    Part,
    /// No label.
    None,
    /// « Total », « Montant », « Amount »…
    Total,
    /// « Total à payer », « Amount due », « Zu zahlen »…
    ToPay,
}

/// A number as a page writes it (spaces of any width inside: « 1 234,56 »).
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d(?:[\d.,' \x{2019}\x{02bc}]*\d)?").expect("number pattern"));
/// A number written as money, with its cents (« 32,87 »), for a total written without its currency mark.
static BARE_MONEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d[\d .,']*[.,]\d{2}$").expect("money pattern"));
/// The codes of an amount written without a currency mark.
pub const BARE: &[&str] = &["?"];
/// A line holding only groups of four digits (« 4242 », « 4242 4242 »): a card's groups shown back.
static CARD_GROUPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*\d{4}(?:[ -]\d{4}){0,4}[ \t]*$").expect("card groups pattern"));
/// A digit run that may be a card number (12 to 19 digits, spaces or dashes between): never read.
static CARD_LIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d(?:[ -]?\d){11,18}").expect("card pattern"));
/// « 27€97 » (the cents right after the mark; `pageText` glues « 27€<sup>97</sup> » so).
static EURO_CENTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d) ?€(\d{2})([^\d.,]|$)").expect("euro cents pattern"));
/// « 27 ,97 », « 32⏎,05 € » (the cents in a piece of their own, on the same line or the next: a flex item, Relecture 6).
static LOOSE_CENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d)[ \n]([.,]\d{2})\b").expect("cents pattern"));
/// A bracketed piece of a line (« (dont 4,66 € de TVA) »).
static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\([^()]*\)").expect("brackets pattern"));
/// An instalment right before the amount (« 3 x 9,32 € », « 3 fois 9,32 € », « 4x € 9,32 »).
static PER_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    // two or more (« 1x », « 1 fois » is the whole total: Relecture 6, u1)
    Regex::new(r"(?i)(?:^|[^\d])(?:[2-9]|[1-9]\d)\s*(?:[x×]|fois|times|mal|veces|cuotas|termijnen|keer)\s*(?:€|eur|\$|£)?\s*$")
        .expect("instalment pattern")
});
/// An instalment right after it (« 9,32 € / mois », « 9,32 € par mois », « $9.32/mo »).
static PER_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:€|eur|euros?|\$|£|usd|gbp|chf)?\s*(?:/|par|per|pro|al|por|a|each|chacun)\s*(?:mois|month|mo\b|monat|mes|maand|semaine|week)")
        .expect("instalment pattern")
});
/// The words right before an amount that make it never charged: a threshold, a limit, an old price (« dès 49 € »,
/// « au lieu de 40 € », « maximum 1 500 € »). Only when they end the label, right before the amount (Relecture 2:
/// « Amount to be taken from your card: €32.87 » or « livraison offerte : 32,87 € » are no mentions).
static MENTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\[barré\]|\[suggestion\]|\bd[èe]s|[àa] partir d[e']|\bfrom|\bab|\bdesde|\bvanaf|au lieu d[e']|instead of|\bstatt|en lugar de|in plaats van|jusqu'?\s?[àa]|up to|bis zu|\bhasta|\bmax|\bmaximum|\bmaximal|plafond(?: de)?|\blimite?(?: de)?|\bavant|\bwas|\bantes)\s*[:\-–]?\s*$",
    )
    .expect("mention pattern")
});
/// A label that says the amount is a part of the total, not the total.
static PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)sous[- ]?total|sub[- ]?total|zwischensumme|subtotaal|articles|produits|\bitems?\b|artikel|productos|hors taxe|\bht\b|\btva\b|\bvat\b|mwst|\biva\b|\bbtw\b|livraison|shipping|delivery|versand|env[ií]o|verzend|remise|discount|[ée]conomi|saving|rabatt|descuento|korting|r[ée]duction|frais|\bfees?\b|gebühr|cargo|kosten|acompte|deposit|anzahlung|aanbetaling",
    )
    .expect("part pattern")
});
/// A label that makes the amount a part whatever total words it holds: before tax (« Montant total HT », « Order
/// total (excl. VAT) », « netto »), a subtotal (Relecture 3, s1).
static PART_ALWAYS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)sous[- ]?total|sub[- ]?total|zwischensumme|subtotaal|\bht\b|hors tax|\bexcl|exkl|\bnetto|\bnet\b|before tax|pre-tax|sans tva|ohne mwst|sin iva|zonder btw|excluding",
    )
    .expect("part pattern")
});
/// A total to pay's label.
static TO_PAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[àa] payer|to pay|\bdue\b|zu zahlen|a pagar|te betalen|[àa] r[ée]gler|grand total|order total|total ttc|montant total|gesamtbetrag|gesamtsumme|endbetrag|importe total|total amount|totaalbedrag",
    )
    .expect("to pay pattern")
});
/// A total's label.
static TOTAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)total|montant|amount|betrag|gesamt|summe|importe|totaal|bedrag").expect("total pattern")
});

/// How far before an amount its label is looked for (characters).
const LABEL_SPAN: usize = 40;

/// The last `n` characters of `text`.
fn last_chars(text: &str, n: usize) -> &str {
    let start = text.char_indices().rev().nth(n.saturating_sub(1)).map_or(0, |(i, _)| i);
    &text[start..]
}

/// The first `n` characters of `text`.
fn first_chars(text: &str, n: usize) -> &str {
    let end = text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    &text[..end]
}

/// A piece of a line that holds only currency marks, punctuation and spaces.
fn mark_only(piece: &str) -> bool {
    let piece = piece.trim();
    piece.chars().all(|c| !c.is_alphanumeric()) || (piece.chars().count() <= 4 && mark_near(piece, "").is_some())
}

/// The words of a line after its last digit (and the marks right after it): a label's own words.
fn after_last_digit(line: &str) -> &str {
    match line.rfind(|c: char| c.is_ascii_digit()) {
        Some(at) => &line[at + 1..],
        None => line,
    }
}

/// The class of an amount whose line, up to it, is `line_before`, the lines above being `above` (latest last).
fn class_of(line_before: &str, above: &[&str]) -> Class {
    // inside brackets: « (dont 4,66 € de TVA) »
    if line_before.matches('(').count() > line_before.matches(')').count() {
        return Class::Part;
    }
    // before tax, a subtotal: a part, whatever else the label says (its brackets included: « (excl. VAT) »)
    let raw_label = {
        let own = after_last_digit(line_before);
        if mark_only(own) {
            above.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty() && !mark_only(l)).unwrap_or("").to_string()
        } else {
            own.to_string()
        }
    };
    // `pageText`'s marks: an old price struck out, a product suggested (anywhere on its piece of text)
    if raw_label.contains("[barré]") || raw_label.contains("[suggestion]") {
        return Class::Mention;
    }
    if PART_ALWAYS.is_match(last_chars(raw_label.trim(), LABEL_SPAN)) && !MENTION.is_match(raw_label.trim()) {
        return Class::Part;
    }
    let mut cleaned = line_before.to_string();
    while BRACKETS.is_match(&cleaned) {
        cleaned = BRACKETS.replace_all(&cleaned, " ").into_owned();
    }
    let mut label = after_last_digit(&cleaned).to_string();
    if mark_only(&label) {
        // the line above, when it holds no amount of its own (« Total à payer: » then « EUR 27.97 »)
        label = above
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !mark_only(l))
            .filter(|l| !l.chars().any(|c| c.is_ascii_digit()))
            .map(|l| BRACKETS.replace_all(l, " ").into_owned())
            .unwrap_or_default();
    }
    let tail = last_chars(label.trim(), LABEL_SPAN);
    // a total to pay's words win over a part's (« Total TTC, livraison offerte : »); a mention only when its words end
    // the label, right before the amount
    if MENTION.is_match(tail) {
        Class::Mention
    } else if TO_PAY.is_match(tail) {
        Class::ToPay
    } else if PART.is_match(tail) {
        Class::Part
    } else if TOTAL.is_match(tail) {
        Class::Total
    } else {
        Class::None
    }
}

/// A page's text made ready to read: lower case, one kind of space, no card-like digit run, « 27€97 » and « 27 ,97 »
/// as « 27,97 € » and « 27,97 ».
fn normalised(text: &str) -> String {
    let text: String = text
        .to_lowercase()
        .chars()
        .map(|c| if matches!(c, '\u{a0}' | '\u{202f}' | '\u{2009}' | '\u{2007}' | '\u{200a}') { ' ' } else { c })
        .collect();
    let text = CARD_LIKE.replace_all(&text, " ");
    // a line of digit groups alone (a card shown back group by group, Relecture 3, c3): never read
    let text = CARD_GROUPS.replace_all(&text, "");
    let text = EURO_CENTS.replace_all(&text, "$1,$2 €$3");
    LOOSE_CENTS.replace_all(&text, "$1$2").into_owned()
}

/// Every amount next to a currency mark in a page's text, in order.
pub fn amounts(text: &str) -> Vec<Amount> {
    let text = normalised(text);
    let mut out = Vec::new();
    for found in NUMBER.find_iter(&text) {
        let (start, end) = (found.start(), found.end());
        let (before, after) = (&text[..start], &text[end..]);
        let mark_before = mark_near(before, "");
        let mark_after = mark_near("", after);
        let Some(codes) = mark_after.or(mark_before) else {
            // « Total à payer : 32,87 », the currency elsewhere (a column's head): a total's number all the same
            if BARE_MONEY.is_match(found.as_str()) {
                if let Some(cents) = amount_of(found.as_str()) {
                    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
                    let above: Vec<&str> =
                        text[..line_start].lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
                    let class = class_of(&text[line_start..start], &above);
                    if class >= Class::Total {
                        out.push(Amount { cents, codes: BARE, class, field: false });
                    }
                }
            }
            continue;
        };
        let whole = found.as_str();
        // « Qté 2 27,97 € »: read from the group next to the mark when the whole doesn't read as one number
        let (number, number_start) = match amount_of(whole) {
            Some(_) => (whole, start),
            None if mark_after.is_some() => match whole.rfind(' ') {
                Some(at) => (&whole[at + 1..], start + at + 1),
                None => continue,
            },
            None => match whole.find(' ') {
                Some(at) => (&whole[..at], start),
                None => continue,
            },
        };
        let Some(cents) = amount_of(number) else { continue };
        let before = &text[..number_start];
        if PER_BEFORE.is_match(last_chars(before, 16)) || PER_AFTER.is_match(first_chars(after, 24)) {
            continue;
        }
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let above: Vec<&str> = text[..line_start].lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
        let line_before = &text[line_start..number_start];
        let field = after_last_digit(line_before).contains("[champ]");
        out.push(Amount { cents, codes, class: class_of(line_before, &above), field });
    }
    out
}

/// What the page's texts (card_fill.rs `Page::page_texts`, plain texts here: wholly visible, the shop's) say of the
/// total of a payment of `cents` in `currency` (« EUR »).
pub fn page_total(texts: &[String], cents: i64, currency: &str) -> PageTotal {
    page_total_of(&texts.iter().map(|t| PageText::plain(t)).collect::<Vec<_>>(), cents, currency)
}

/// An amount and where it was read.
struct Read {
    amount: Amount,
    provider: bool,
    /// Read in a closed shadow root (`PageText::closed`).
    closed: bool,
    /// The frame's place in the texts (for the log).
    frame: usize,
}

fn read_all(texts: &[PageText], shown: bool) -> Vec<Read> {
    texts
        .iter()
        .enumerate()
        .flat_map(|(frame, t)| {
            amounts(if shown { &t.shown } else { &t.all })
                .into_iter()
                .map(move |amount| Read { amount, provider: t.provider, closed: t.closed, frame })
        })
        .collect()
}

/// Where a doubt was seen, for the log (never the page's text): the frame's place among the texts and whether it is
/// the provider's.
pub fn doubt_where(texts: &[PageText], doubt: &Doubt) -> Option<(usize, bool)> {
    let cents = match doubt {
        Doubt::Higher(c) | Doubt::Foreign(c, _) => *c,
        Doubt::Unclear => return None,
    };
    read_all(texts, false).into_iter().find(|r| r.amount.cents == cents).map(|r| (r.frame, r.provider))
}

/// The total to name among `labelled` (visible totals and totals to pay): the provider's first, a total to pay before
/// a total, the largest.
fn pick(labelled: &[&Read]) -> Option<i64> {
    let rank = |r: &&&Read| (r.provider, r.amount.class == Class::ToPay, r.amount.cents);
    labelled.iter().max_by_key(rank).map(|r| r.amount.cents)
}

/// What the page's texts say of the total of a payment of `cents` in `currency` (the rule: this module's head).
pub fn page_total_of(texts: &[PageText], cents: i64, currency: &str) -> PageTotal {
    let shown = read_all(texts, true);
    let all = read_all(texts, false);
    let marked = |r: &Read| r.amount.codes.contains(&currency);
    // a total without its currency mark counts in the payment's currency when the page writes that one elsewhere
    let uses_currency = all.iter().any(&marked);
    let bare = |r: &Read| r.amount.codes == BARE;
    let ours = |r: &&Read| marked(r) || (bare(r) && uses_currency);
    let shown_ours: Vec<&Read> = shown.iter().filter(ours).collect();
    let all_ours: Vec<&Read> = all.iter().filter(ours).collect();
    let foreign: Vec<&Read> = all.iter().filter(|r| !marked(r) && !bare(r)).collect();
    let seen = || Seen {
        amounts: all_ours.iter().map(|r| r.amount.cents).collect(),
        labelled: all_ours.iter().filter(|r| r.amount.class >= Class::Total).count(),
        other_currency: foreign.len(),
    };
    let labelled: Vec<&Read> = shown_ours.iter().copied().filter(|r| r.amount.class >= Class::Total).collect();
    // 1. the page asks more: a visible total above the approved amount, or the provider's own visible amount above it
    let above: Vec<&Read> = labelled.iter().copied().filter(|r| r.amount.cents > cents).collect();
    if let Some(total) = pick(&above) {
        return PageTotal::Other(total);
    }
    let provider_above = shown_ours
        .iter()
        .filter(|r| r.provider && r.amount.class != Class::Mention && r.amount.cents > cents)
        .map(|r| r.amount.cents)
        .max();
    if let Some(total) = provider_above {
        return PageTotal::Other(total);
    }
    // 2. seen, so never left to the agent: another currency, an amount above the approved one, a total without its
    // currency that is not the approved amount
    if let Some(r) = foreign.iter().filter(|r| r.amount.class != Class::Mention).max_by_key(|r| (r.provider, r.amount.cents)) {
        return PageTotal::Doubt(Doubt::Foreign(r.amount.cents, r.amount.codes[0]));
    }
    // an amount above it: visible (not a mention), or hidden but labelled as a total, or anywhere in the provider's own
    // frame or page (never relaxed there); hidden product prices (a menu, a drawer) are no doubt (Relecture 3, c5)
    let blocks = |r: &&&Read| {
        r.amount.cents > cents
            && (r.closed
                || (r.amount.field && r.amount.class >= Class::None)
                || (r.amount.class != Class::Mention && (r.amount.class >= Class::Total || r.provider)))
    };
    let visible_higher = shown_ours.iter().filter(|r| r.amount.cents > cents && r.amount.class != Class::Mention);
    if let Some(higher) = visible_higher.chain(all_ours.iter().filter(blocks)).map(|r| r.amount.cents).max() {
        return PageTotal::Doubt(Doubt::Higher(higher));
    }
    if all.iter().any(|r| bare(r) && r.amount.cents != cents && !uses_currency) {
        return PageTotal::Doubt(Doubt::Unclear);
    }
    // 3. the provider's own amounts and every total to pay must be the approved one
    let provider: Vec<i64> = shown_ours.iter().filter(|r| r.provider && marked(r)).map(|r| r.amount.cents).collect();
    let provider_agrees = provider.is_empty() || provider.contains(&cents);
    let to_pay_agree = shown_ours.iter().filter(|r| r.amount.class == Class::ToPay).all(|r| r.amount.cents == cents);
    if !provider_agrees || !to_pay_agree {
        return PageTotal::Doubt(Doubt::Unclear);
    }
    // 4. beyond doubt (and the whole page read): the approved amount is the total
    let labelled_approved = labelled.iter().any(|r| r.amount.cents == cents && marked(r));
    let largest_unlabelled = labelled.is_empty()
        && shown_ours.iter().any(|r| r.amount.cents == cents && marked(r))
        && shown_ours.iter().all(|r| r.amount.cents <= cents);
    let whole = texts.iter().all(|t| !t.truncated);
    if whole && (labelled_approved || largest_unlabelled) {
        return PageTotal::Approved;
    }
    // 5. the approved amount nowhere, one clear visible total (below it)
    if !all_ours.iter().any(|r| r.amount.cents == cents) {
        let mut values: Vec<i64> = labelled.iter().map(|r| r.amount.cents).collect();
        values.sort_unstable();
        values.dedup();
        if values.len() == 1 {
            return PageTotal::Other(values[0]);
        }
    }
    PageTotal::Unread(seen())
}

/// The total the agent gives `confirm_payment` (« 27,97 € », « EUR 27.97 », « 27.97 »), read with the same parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stated {
    /// In the payment's currency (or written without a currency mark), cents.
    Amount(i64),
    /// In another currency (cents, code).
    Foreign(i64, &'static str),
    /// Not one clear amount.
    Unreadable,
}

/// The agent's `total` for a payment in `currency`: the bridge compares the number itself (Relecture 2: the model never
/// compares amounts).
pub fn stated_total(text: &str, currency: &str) -> Stated {
    let marked: Vec<Amount> = amounts(text).into_iter().filter(|a| a.codes != BARE).collect();
    let mut values: Vec<(i64, bool)> = marked.iter().map(|a| (a.cents, a.codes.contains(&currency))).collect();
    values.sort_unstable();
    values.dedup();
    match (values.as_slice(), marked.first()) {
        ([(cents, true)], _) => Stated::Amount(*cents),
        ([(cents, false)], Some(a)) => Stated::Foreign(*cents, a.codes[0]),
        ([], _) => {
            let plain = text.trim();
            let only_number = !plain.is_empty() && plain.chars().all(|c| c.is_ascii_digit() || " .,'\u{a0}\u{202f}".contains(c));
            match amount_of(&plain.replace(['\u{a0}', '\u{202f}'], " ")).filter(|_| only_number) {
                Some(cents) => Stated::Amount(cents),
                None => Stated::Unreadable,
            }
        }
        _ => Stated::Unreadable,
    }
}

/// What the press does once the page's total is read again right before it (card_pay.rs `confirm`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recheck {
    Press,
    /// Not « approved » as it was before the card was typed: given up, with no number at all (what appeared after the
    /// card was typed may be the card shown back: Relecture 3, c3).
    Changed,
}

/// `before`: the total read before the card was typed (only « approved » reaches the press); `now`: read again right
/// before the press.
pub fn recheck(before: &PageTotal, now: &PageTotal) -> Recheck {
    if *before == PageTotal::Approved && *now == PageTotal::Approved {
        Recheck::Press
    } else {
        Recheck::Changed
    }
}

/// Correction 1 (19/11): what is visible, the provider's own amounts, other currencies, mentions.
#[cfg(test)]
mod correction_1 {
    use super::*;

    fn text(shown: &str, all: &str, provider: bool) -> PageText {
        PageText { shown: shown.into(), all: all.into(), provider, truncated: false, closed: false }
    }

    #[test]
    fn only_visible_text_supports_approved_and_any_laid_out_amount_above_blocks_it() {
        // the visible total is higher; an off-screen « to pay » line says the approved amount
        let t = text("Total 35,00 €", "Total 35,00 €\nTotal à payer 27,97 €", false);
        assert_eq!(page_total_of(&[t], 2797, "EUR"), PageTotal::Other(3500));
        // the visible total is the approved one; an off-screen higher total: never approved
        let t = text("Total à payer 27,97 €", "Total à payer 27,97 €\nTotal 35,00 €", false);
        assert_eq!(page_total_of(&[t], 2797, "EUR"), PageTotal::Doubt(Doubt::Higher(3500)));
        // the approved total only off-screen: not visible, not approved
        let t = text("Numéro de carte", "Total à payer 27,97 €\nNuméro de carte", false);
        assert!(matches!(page_total_of(&[t], 2797, "EUR"), PageTotal::Unread(_)));
    }

    #[test]
    fn the_providers_own_amount_must_be_the_approved_one() {
        let shop = text("Total à payer 27,97 €", "Total à payer 27,97 €", false);
        let agrees = text("Montant : 27,97 EUR\nNuméro de carte", "Montant : 27,97 EUR\nNuméro de carte", true);
        assert_eq!(page_total_of(&[shop.clone(), agrees], 2797, "EUR"), PageTotal::Approved);
        let silent = text("Numéro de carte\nCVC", "Numéro de carte\nCVC", true);
        assert_eq!(page_total_of(&[shop.clone(), silent], 2797, "EUR"), PageTotal::Approved);
        let higher = text("Montant : 35,00 EUR", "Montant : 35,00 EUR", true);
        assert_eq!(page_total_of(&[shop.clone(), higher], 2797, "EUR"), PageTotal::Other(3500));
        // the provider shows something else than the approved amount (only a fee): not beyond doubt
        let fee = text("Frais : 1,50 EUR", "Frais : 1,50 EUR", true);
        assert_eq!(page_total_of(&[shop, fee], 2797, "EUR"), PageTotal::Doubt(Doubt::Unclear));
    }

    #[test]
    fn a_total_in_another_currency_is_never_approved() {
        let page = "Total à payer 27,97 €\nMontant débité : USD 32.10";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Doubt(Doubt::Foreign(3210, "USD")));
        let t = text("Total à payer 27,97 €", "Total à payer 27,97 €", false);
        let provider = text("32,10 USD", "32,10 USD", true);
        assert_eq!(page_total_of(&[t, provider], 2797, "EUR"), PageTotal::Doubt(Doubt::Foreign(3210, "USD")));
    }

    #[test]
    fn thresholds_limits_and_old_prices_are_never_charged() {
        let page = "Total 27,97 € au lieu de 32,00 €\nLivraison offerte dès 49,00 €\nMontant maximum 1 500,00 €";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn the_euro_sign_between_euros_and_cents() {
        assert_eq!(total(&["Total TTC 27€97"], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total(&["Total TTC 27€97\nLivraison 4,66 € 3 articles"], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total(&["Total TTC 27€97"], 2000, "EUR"), PageTotal::Other(2797));
    }

    #[test]
    fn the_press_waits_for_the_same_total() {
        let unread = PageTotal::Unread(Seen::default());
        assert_eq!(recheck(&PageTotal::Approved, &PageTotal::Approved), Recheck::Press);
        assert_eq!(recheck(&PageTotal::Approved, &PageTotal::Other(2947)), Recheck::Changed);
        assert_eq!(recheck(&PageTotal::Approved, &unread), Recheck::Changed);
        assert_eq!(recheck(&PageTotal::Approved, &PageTotal::Doubt(Doubt::Higher(3000))), Recheck::Changed);
        assert_eq!(recheck(&unread, &PageTotal::Approved), Recheck::Changed);
        // the fee of Relecture 1 (Chrome r3): « Total à payer: EUR 27.97 » becomes « EUR 29.47 » after the number
        let before = page_total(&["Total à payer:\nEUR 27.97".to_string()], 2797, "EUR");
        let now = page_total(&["Frais carte : EUR 1.50\nTotal à payer:\nEUR 29.47".to_string()], 2797, "EUR");
        assert_eq!(recheck(&before, &now), Recheck::Changed);
    }

    #[test]
    fn correction_6_a_read_only_field_above_the_approved_amount_blocks() {
        let page = PageText { shown: "Total à payer: EUR 27.97".into(), all: "Total à payer: EUR 27.97\n[champ] 32,87 €".into(), ..Default::default() };
        assert_eq!(page_total_of(&[page], 2797, "EUR"), PageTotal::Doubt(Doubt::Higher(3287)));
        let promo = PageText { shown: "Total à payer: EUR 27.97".into(), all: "Total à payer: EUR 27.97\n[champ] Remise 99,99 €".into(), ..Default::default() };
        assert_eq!(page_total_of(&[promo], 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn correction_5_an_amount_above_the_approved_one_in_a_closed_root_blocks_whatever_its_label() {
        let shop = PageText::plain("Montant de votre panier : 27,97 €\nTotal à payer");
        let closed = PageText { all: "32,87 €".into(), closed: true, ..Default::default() };
        assert_eq!(page_total_of(&[shop.clone(), closed], 2797, "EUR"), PageTotal::Doubt(Doubt::Higher(3287)));
        // a closed widget without amounts, or with a lower one: nothing blocked
        let help = PageText { all: "Besoin d'aide ?\n4,90 €".into(), closed: true, ..Default::default() };
        let page = PageText::plain("Total à payer: EUR 27.97");
        assert_eq!(page_total_of(&[page, help], 2797, "EUR"), PageTotal::Approved);
        // another site's frame (read as not shown): its unlabelled amounts are ignored, a labelled higher total blocks
        let frame = |text: &str| PageText { all: text.into(), ..Default::default() };
        let page = PageText::plain("Total à payer: EUR 27.97");
        assert_eq!(page_total_of(&[page.clone(), frame("protected by reCAPTCHA\nPayPal 49,90 €")], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(page_total_of(&[page, frame("Total à payer 32,87 €")], 2797, "EUR"), PageTotal::Doubt(Doubt::Higher(3287)));
    }

    #[test]
    fn correction_3_struck_prices_suggestions_hidden_menus_card_groups_and_before_tax() {
        // struck-out old price (`pageText` marks it « [barré] »), a product suggestion (« [suggestion] »)
        let page = "Crème solaire SPF50 [barré] 35,00 € 27,97 €\n[suggestion] Coffret soin 49,90 €\nTotal à payer 27,97 €";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Approved);
        // a hidden menu with prices blocks nothing; a hidden total does; the provider's hidden amounts always do
        let menu = PageText { shown: "Total à payer 27,97 €".into(), all: "Coffret soin 49,90 €\nTotal à payer 27,97 €".into(), ..Default::default() };
        assert_eq!(page_total_of(&[menu], 2797, "EUR"), PageTotal::Approved);
        let hidden_total = PageText { shown: "Total à payer 27,97 €".into(), all: "Total à payer 27,97 €\nTotal 32,87 €".into(), ..Default::default() };
        assert_eq!(page_total_of(&[hidden_total], 2797, "EUR"), PageTotal::Doubt(Doubt::Higher(3287)));
        let shop = PageText::plain("Total à payer 27,97 €");
        let provider = PageText { shown: "Numéro de carte".into(), all: "Coffret 49,90 €\nNuméro de carte".into(), provider: true, ..Default::default() };
        assert_eq!(page_total_of(&[shop, provider], 2797, "EUR"), PageTotal::Doubt(Doubt::Higher(4990)));
        // a card shown back group by group under the amount: never an amount
        assert_eq!(total(&["Total à payer 27,97 €\n4242\n4242\n4242\n4242"], 2797, "EUR"), PageTotal::Approved);
        // before tax stays a part
        assert_eq!(total(&["Montant total HT 23,31 €\nTVA 4,66 €\nMontant total TTC 27,97 €"], 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn the_agents_total_is_compared_by_the_bridge() {
        for written in ["27,97 €", "EUR 27.97", "27.97 EUR", "€27.97", "27,97", "27.97", " 27,97\u{a0}€ "] {
            assert_eq!(stated_total(written, "EUR"), Stated::Amount(2797), "{written:?}");
        }
        assert_eq!(stated_total("32,10 USD", "EUR"), Stated::Foreign(3210, "USD"));
        for written in ["", "le total", "27,97 € ou 32,87 €", "27 97"] {
            assert_eq!(stated_total(written, "EUR"), Stated::Unreadable, "{written:?}");
        }
    }

    #[test]
    fn a_seen_problem_is_never_left_to_the_agent() {
        // an unlabelled amount above the approved one, another currency, a total without its currency mark
        assert_eq!(
            total(&["Doliprane 3,50 €\nSpray 24,47 €\nCrème 30,00 €"], 2447, "EUR"),
            PageTotal::Doubt(Doubt::Higher(3000))
        );
        assert_eq!(
            total(&["Total à payer 27,97 €\nVous serez débité de 32,10 USD"], 2797, "EUR"),
            PageTotal::Doubt(Doubt::Foreign(3210, "USD"))
        );
        assert_eq!(total(&["Montant (EUR)\nTotal à payer : 32,87"], 2797, "EUR"), PageTotal::Doubt(Doubt::Unclear));
        // the whole page not read (`pageText`'s cap): never approved
        let cut = PageText { truncated: true, ..PageText::plain("Total à payer 27,97 €") };
        assert!(matches!(page_total_of(&[cut], 2797, "EUR"), PageTotal::Unread(_)));
        // a mention only when its words qualify the amount itself
        assert_eq!(total(&["Total 27,97 €\nLivraison offerte dès 49,00 €"], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total(&["Total panier 27,97 €\nTotal TTC, livraison offerte : 32,87 €"], 2797, "EUR"), PageTotal::Other(3287));
    }

    #[test]
    fn a_card_number_shown_back_is_never_read() {
        let PageTotal::Unread(seen) = total(&["Total : €\n4242 4242 4242 4242"], 1250, "EUR") else { panic!() };
        assert!(seen.amounts.is_empty(), "{seen:?}");
        assert_eq!(total(&["Total 12,50 €\n4242424242424242\n12/30"], 1250, "EUR"), PageTotal::Approved);
    }

    fn total(texts: &[&str], cents: i64, currency: &str) -> PageTotal {
        page_total(&texts.iter().map(|t| t.to_string()).collect::<Vec<_>>(), cents, currency)
    }
}

#[cfg(test)]
mod review_1 {
    use super::*;

    fn total(texts: &[&str], cents: i64, currency: &str) -> PageTotal {
        page_total(&texts.iter().map(|t| t.to_string()).collect::<Vec<_>>(), cents, currency)
    }

    #[test]
    fn r1_two_to_pay_totals_that_disagree_are_never_approved() {
        // the shop's page and the provider's frame each say what is due; the provider's is higher
        let got = total(&["Votre commande\nTotal à payer 27,97 €", "Montant à régler 35,00 €\nNuméro de carte"], 2797, "EUR");
        assert_ne!(got, PageTotal::Approved, "two « to pay » totals, 27,97 and 35,00: approved 27,97");
    }

    #[test]
    fn r2_a_higher_labelled_total_beside_the_approved_one_is_never_approved() {
        // the agent approved the basket and forgot the shipping: the very case the check is for
        let got = total(&["Total panier 27,97 €\nFrais de livraison 4,90 €\nTotal 32,87 €"], 2797, "EUR");
        assert_ne!(got, PageTotal::Approved, "« Total 32,87 € » on the page: approved 27,97");
        // a card fee added by the provider, both lines « Total »
        let got = total(&["Total €27.97\nCard fee €0.50\nTotal €28.47"], 2797, "EUR");
        assert_ne!(got, PageTotal::Approved, "« Total €28.47 » on the page: approved 27,97");
    }

    #[test]
    fn r3_the_providers_own_amount_is_not_outvoted_by_the_shops_page() {
        // the shop's page (any text it likes) says « Total à payer 27,97 € », the provider's frame « Montant 35,00 EUR »
        let got = total(&["Total à payer 27,97 €", "Montant : 35,00 EUR\nNuméro de carte\nPayer"], 2797, "EUR");
        assert_ne!(got, PageTotal::Approved, "the provider's frame says 35,00 EUR: approved 27,97");
    }

    #[test]
    fn r4_the_27_euros_97_way_of_writing_is_not_another_total() {
        // « 27€97 » (Cdiscount, Leclerc, Carrefour, Fnac…): read as 27,00 € and 97,00 €, the payment cancelled and the
        // person told « la page demandait 27,00 € »
        let got = total(&["Total TTC 27€97"], 2797, "EUR");
        assert!(!matches!(got, PageTotal::Other(_)), "« Total TTC 27€97 » for 27,97 approved: {got:?}");
    }

    #[test]
    fn r5_the_vat_inside_the_total_line_is_not_the_total() {
        let got = total(&["Total TTC (dont 4,66 € de TVA) 27,97 €"], 2797, "EUR");
        assert!(!matches!(got, PageTotal::Other(_)), "VAT 4,66 € read as the total: {got:?}");
    }

    #[test]
    fn r6_an_amount_whose_cents_are_a_separate_piece_is_not_another_total() {
        // `pageText` joins « <span>27</span><span>,97 €</span> » with a space (« ,97 € » is not only digits)
        let got = total(&["Total 27 ,97 €"], 2797, "EUR");
        assert!(!matches!(got, PageTotal::Other(_)), "« Total 27 ,97 € » for 27,97 approved: {got:?}");
    }

    #[test]
    fn r7_a_card_number_echoed_by_the_page_never_reaches_the_log() {
        // a card preview under an amount (« 27,97 € » then the number the page shows back): Seen is logged as is
        // (Correction 2: the 27,97 € above the approved 12,50 is now a seen doubt, logged as such: never the card's digits)
        let got = total(&["27,97 €\n4242 4242 4242 4242\n12/30"], 1250, "EUR");
        assert!(!format!("{got:?}").contains("4242"), "the card's first four digits in the log line: {got:?}");
        assert_eq!(got, PageTotal::Doubt(Doubt::Higher(2797)));
    }

    #[test]
    fn r8_a_misread_total_approved_again_is_never_the_page_s_total() {
        // total_differs tells the agent to offer the misread amount; approved again, it is then « the page's total »
        // and confirm presses « Payer », which pays the real 27,97 €
        assert_ne!(total(&["Total TTC (dont 4,66 € de TVA) 27,97 €"], 466, "EUR"), PageTotal::Approved);
        assert_ne!(total(&["Total TTC 27€97"], 2700, "EUR"), PageTotal::Approved);
    }
}

/// Relecture 2 (19/11, on 52c025772): each asserts the SAFE outcome; they fail on 52c025772.
#[cfg(test)]
mod review_2 {
    use super::*;

    fn text(shown: &str, provider: bool) -> PageText {
        PageText { shown: shown.into(), all: shown.into(), provider, truncated: false, closed: false }
    }

    #[test]
    fn q1_the_providers_own_visible_amount_above_the_approved_one_stops_the_payment() {
        // Stripe Checkout-like: « Pharmacie des Drakkars / €32.87 » unlabelled in the provider's page, the shop's cart
        // (approved, shipping forgotten) 27,97 €. The bridge SAW the provider ask more, yet returns Unread, i.e. the
        // agent's own comparison (« If the total you read before was 27.97 EUR, call confirm_payment »).
        let shop = text("Total panier 27,97 €", false);
        let provider = text("Pharmacie des Drakkars\n€32.87\nCard information", true);
        assert_eq!(page_total_of(&[shop, provider], 2797, "EUR"), PageTotal::Other(3287));
    }

    #[test]
    fn q2_another_currency_on_the_providers_page_is_never_left_to_the_agent() {
        // DCC preselected: the provider will debit 32,10 USD; the button says « Payer ». Unread = the agent compares
        // « 27,97 EUR » with what it read before and confirms.
        let provider = text("Montant : 27,97 EUR\nVous serez débité de 32,10 USD\nNuméro de carte", true);
        let got = page_total_of(&[provider], 2797, "EUR");
        assert!(!matches!(got, PageTotal::Unread(_) | PageTotal::Approved), "DCC left to the agent: {got:?}");
    }

    #[test]
    fn q3_a_real_total_whose_label_holds_a_mention_word_is_not_ignored() {
        // « offerte », « gratuit », « from » make the higher total a « mention »: ignored, the basket total approved
        for page in [
            "Total panier 27,97 €\nTotal TTC, livraison offerte : 32,87 €",
            "Total panier 27,97 €\nTotal à payer avec livraison gratuite : 32,87 €",
            "Order total €27.97\nAmount to be taken from your card: €32.87",
        ] {
            assert_ne!(page_total(&[page.to_string()], 2797, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn q4_a_total_to_pay_written_without_its_currency_mark_is_not_ignored() {
        // the currency in a column header (« Montant (EUR) ») or nowhere: the « to pay » number is skipped
        let page = "Total panier 27,97 €\nFrais de port 4,90 €\nTotal à payer : 32,87";
        assert_ne!(page_total(&[page.to_string()], 2797, "EUR"), PageTotal::Approved);
    }
}

/// Relecture 3 (19/11, on a502ce997): each asserts the expected outcome; they fail on a502ce997.
#[cfg(test)]
mod review_3 {
    use super::*;

    #[test]
    fn s1_a_total_before_tax_labelled_total_is_not_a_second_total_to_pay() {
        // « to pay » words now win over a part's: « Montant total HT » is read as a total to pay of 23,31 €, which
        // disagrees with the approved 27,97 € → Doubt::Unclear → a good payment stopped (B2B and many EU pages)
        for page in [
            "Montant total HT 23,31 €\nTVA 4,66 €\nMontant total TTC 27,97 €",
            "Order total (excl. VAT) €23.31\nVAT €4.66\nOrder total €27.97",
            "Grand total excl. VAT €23.31\nVAT €4.66\nGrand total €27.97",
        ] {
            assert_eq!(page_total(&[page.to_string()], 2797, "EUR"), PageTotal::Approved, "{page}");
        }
    }
}

/// Relecture 6 (19/11, on ea52bd0fa): each asserts the expected outcome; they fail on ea52bd0fa.
#[cfg(test)]
mod review_6 {
    use super::*;

    #[test]
    fn u1_paying_in_one_instalment_is_the_whole_total_not_an_instalment() {
        // Alma / Oney / Floa choice « Payer en 1x » (the whole amount) beside « 3x sans frais »: PER_BEFORE takes « 1x »
        // for an instalment and drops the real total, the basket line reads « approved »
        for page in [
            "Montant de votre panier : 27,97 €\nPayer en 1x\n32,87 €\nPayer en 3x sans frais\n3 x 10,96 €",
            "Montant de votre panier : 27,97 €\nPayer en 1x 32,87 €\nPayer en 3x 10,96 €",
            "Montant de votre panier : 27,97 €\nPaiement en 1 fois 32,87 €",
        ] {
            assert_ne!(page_total(&[page.to_string()], 2797, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn u2_a_total_whose_cents_sit_on_a_line_of_their_own_is_not_read_as_its_cents() {
        // pageText's lines for « 32,05 € » drawn as flex items (« 32 » / « ,05 € », Chrome f1): read as 5,00 €
        let page = "Montant de votre panier : 27,97 €\nFrais de livraison 4,08 €\nTotal à payer\n32\n,05 €";
        assert_ne!(page_total(&[page.to_string()], 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn u3_euros_then_superscript_mark_and_cents_is_one_amount() {
        // « 156<sup>€90</sup> » (French marketplaces' style), joined by pageText as « 156 €90 »: read 156,00 € and
        // 90,00 € → « la page demandait 156,00 € » for a right payment of 156,90 €
        assert_eq!(page_total(&["Total à payer 156 €90".to_string()], 15690, "EUR"), PageTotal::Approved);
    }
}
