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
//! How the amounts are read, in each text (Relecture 7: fail-safe — over-reading can only hand a payment over):
//! - every number next to a currency mark (cards.rs `mark_near`: « € », « EUR », « euros », « $ »…, before or after it,
//!   spaces of any width between) read as shops write it (cards.rs `amount_of`: « 27,97 », « 27.97 », « 1 234,56 »,
//!   « 1.234,56 », « 1,234.56 »), « 27€97 », « 27 ,97 € » and cents on the next line too; a number that doesn't read
//!   as one is read at its largest plausible value (« 32 05 € » = 3 205 €, « 3 2 , 8 7 € » = 32,87 €), never as its
//!   last group, except a quantity before a whole price with its cents (« Qté 2 27,97 € »); a digit run of 12 or more
//!   or a line of 4-digit groups (a card shown back) is never read;
//! - an instalment (« 3 x 9,32 € », « 3 fois 9,32 € », « 9,32 € / mois »; « 1x » is the total) is left out;
//! - each amount gets a class from its own words, on its line right before it, after any amount before it, brackets
//!   left out: a total to pay (« Total à payer », « Amount due », « Zu zahlen », a pay button's « Payer »…), a total
//!   (« Total », « Montant », « Betrag »…), a part (« Sous-total », « Livraison », « HT », « TVA », « Total articles »,
//!   anything in brackets: « (dont 4,66 € de TVA) »), a mention never charged (« dès 49 € », « jusqu'à », « maximum »,
//!   « au lieu de », its own strike-through or suggestion mark from page_snapshot.rs), or none. The line above, when the
//!   amount's own line has no words, may only name it a total or a total to pay — never an exemption.
//!
//! Each text comes twice: what is really visible and everything laid out (page_snapshot.rs). Then, with the payment's
//! currency:
//! 1. a visible total or total to pay above the approved amount: `Other` (the page asks more), the payment
//!    provider's own frame or page first, a total to pay before a total;
//! 2. any laid-out amount above the approved one that blocks (visible and not a mention; hidden but labelled; the
//!    provider's; in a closed root; a read-only field's), or any amount in another currency that is not a mention
//!    (DCC): `Doubt`;
//! 3. a provider's frame or page that shows amounts must show the approved one, and every visible total to pay must be
//!    the approved one; then only the approved amount, visible and labelled as the total, its currency written, the
//!    whole page read: `Approved` (never an unlabelled largest amount);
//! 4. the approved amount nowhere on the page and one clear visible total (below it): `Other`;
//! 5. else `Unread`.

use std::sync::LazyLock;

use regex::Regex;
use zeroize::Zeroizing;

use crate::cards::{amount_of, mark_near};

/// One frame's text as page_snapshot.rs reads it (what Chrome rendered).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageText {
    /// What is really visible.
    pub shown: String,
    /// Everything laid out (off-screen, clipped, transparent or tiny text included; never `display: none`).
    pub all: String,
    /// The payment provider's own frame or page (CAWL's page, Stripe's frame…), not the shop's.
    pub provider: bool,
    /// The frame's text was longer than what is read (page_snapshot.rs `MAX_CHARS`): never « approved » (Relecture 2, 3).
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
    /// The value of a read-only field (page_snapshot.rs marks it « [champ] »): any one above the approved amount blocks
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
/// « 27€97 » (the cents right after the mark; the rendered boxes « 27 » « €97 » touch so).
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
    // whole words, on the amount's own line right after it (Relecture 8, p1: « €⏎Promo code » is no « per month »)
    Regex::new(r"(?i)^[ \t]*(?:€|eur|euros?|\$|£|usd|gbp|chf)?[ \t]*(?:(?:/[ \t]*|\b(?:par|per|pro|al|por|a|each|chacun)[ \t]+)(?:mois|month|mo|monat|mes|maand|semaine|week)\b|(?:mensuel(?:le)?s?|monthly|monatlich|mensual(?:es)?|maandelijks)\b)")
        .expect("instalment pattern")
});
/// The words right before an amount that make it never charged: a threshold, a limit, an old price (« dès 49 € »,
/// « au lieu de 40 € », « maximum 1 500 € »). Only when they end the label, right before the amount (Relecture 2:
/// « Amount to be taken from your card: €32.87 » or « livraison offerte : 32,87 € » are no mentions).
static MENTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\[barré\]|\[suggestion\]|\bd[èe]s|[àa] partir d[e']|\bfrom|\bab|\bdesde|\bvanaf|au lieu d[e']|instead of|\bstatt|en lugar de|in plaats van|jusqu'?\s?[àa]|up to|bis zu|\bhasta|\bmax|\bmaximum|\bmaximal|plafond(?: de)?|\blimite?(?: de)?|\bavant|\bwas|\bantes|disponible|available|verf[üu]gbar|beschikbaar)\s*[:\-–]?\s*(?:€|\$|£|eur|usd|gbp|chf)?\s*$",
    )
    .expect("mention pattern")
});
/// A threshold's words right before an amount (« supérieure à 49 € », « over €50 », « ab einem Bestellwert von »): a
/// mention only on a line that speaks of free delivery (Relecture 9: « Total à payer, plus de 32,87 € » is no mention).
/// Never « saldo » alone: « Te betalen saldo », « Offener Saldo », « Restsaldo » are the amount due (Relecture 9, y1).
static THRESHOLD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:sup[ée]rieure? [àa]|\bover|\babove|more than|plus de|bestellwert(?: von)?|m[áa]s de|meer dan)\s*[:\-–]?\s*(?:€|\$|£|eur|usd|gbp|chf)?\s*$",
    )
    .expect("threshold pattern")
});
/// Free delivery's words: what makes a threshold's amount a mention.
static FREE_DELIVERY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:livraison|frais de port|port)\s+(?:offert|gratuit)|(?:offert|gratuit)e?s?\b.*(?:livraison|port)|free (?:shipping|delivery)|(?:shipping|delivery) (?:is )?free|versandkostenfrei|kostenlose[rn]? (?:versand|lieferung)|env[ií]o gratis|gratis (?:env[ií]o|verzending|bezorging)|gratis verzonden|gratis bezorgd",
    )
    .expect("free delivery pattern")
});

/// Whether the words `tail` right before an amount, on its line `line`, make it a mention.
fn is_mention(tail: &str, line: &str) -> bool {
    MENTION.is_match(tail) || (THRESHOLD.is_match(tail) && FREE_DELIVERY.is_match(line))
}
/// A label that says the amount is a part of the total, not the total.
static PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)sous[- ]?total|sub[- ]?total|zwischensumme|subtotaal|articles|produits|\bitems?\b|artikel|productos|hors taxe|\bht\b|\btva\b|\bvat\b|mwst|\biva\b|\bbtw\b|livraison|shipping|delivery|versand|env[ií]o|verzend|remise|discount|[ée]conomi|saving|rabatt|descuento|korting|r[ée]duction|frais|\bfees?\b|gebühr|cargo|kosten|acompte|deposit|anzahlung|aanbetaling|משלוח|מע״מ|מע\x22מ|הנחה|דמי",
    )
    .expect("part pattern")
});
/// A label that makes the amount a part whatever total words it holds: before tax (« Montant total HT », « Order
/// total (excl. VAT) », « netto »), a subtotal (Relecture 3, s1).
static PART_ALWAYS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)sous[- ]?total|sub[- ]?total|zwischensumme|subtotaal|\bht\b|hors tax|\bexcl|exkl|\bnetto|before tax|pre-tax|sans tva|ohne mwst|sin iva|zonder btw|excluding|ביניים|לפני מע",
    )
    .expect("part pattern")
});
/// A total to pay's label.
static TO_PAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)net [àa] payer|[àa] payer|to pay|\bdue\b|zu zahlen|a pagar|te betalen|[àa] r[ée]gler|grand total|order total|total ttc|montant total|gesamtbetrag|gesamtsumme|endbetrag|importe total|total amount|totaalbedrag|לתשלום",
    )
    .expect("to pay pattern")
});
/// What a total includes (« inkl. MwSt. », « incl. VAT », « TVA incluse », « IVA incluido », « incl. btw »,
/// « livraison comprise »): left out of a label, so that it never makes the total a part.
static INCLUDED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:incl\.?|inkl\.?|including|inclusief|y?\s?compris)\s+(?:la\s+|de\s+)?(?:tva|vat|mwst\.?|ust\.?|iva|btw|tax(?:es)?|livraison|shipping|versand|frais de port)\b\.?|\b(?:tva|vat|mwst|iva|btw|taxes?|livraison|frais de port)\s+(?:incluse?s?|comprise?s?|incluid[oa]s?|inbegrepen|included)\b",
    )
    .expect("included pattern")
});
/// A pay button's word right before its amount (« Payer 12,50 € », « Pay $34.90 »): what will be charged.
static PAY_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|\s)(?:payer|pay|bezahlen|pagar|betalen|r[ée]gler)\s*(?:€|\$|£|eur|usd|gbp|chf)?\s*[:\-–]?\s*$")
        .expect("pay word pattern")
});
/// A hosted payment page's heading right above its amount (Stripe Checkout's « Pay Pharmacie Exemple » then
/// « €32.87 »): the amount to pay. Never « Pay later », « Pay in 4 », « Pay with card », « Payer en 3 fois ».
static PAY_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:pay|payer|bezahlen an|pagar a|betalen aan)\s+(\S+)").expect("pay heading pattern")
});
/// The words after « Pay » that make it no shop's name.
const NOT_A_SHOP: &[&str] = &[
    "later", "in", "with", "by", "over", "after", "now", "monthly", "en", "par", "avec", "plus", "après", "apres",
    "mit", "später", "spater", "con", "más", "mas", "met", "the", "le", "la", "les", "your", "votre",
];

/// Whether `line` is a hosted page's « Pay <shop> » heading.
fn pay_heading(line: &str) -> bool {
    PAY_HEADING
        .captures(line.trim())
        .and_then(|c| c.get(1))
        .is_some_and(|w| !NOT_A_SHOP.contains(&w.as_str().to_lowercase().as_str()))
}
/// A payment's words anywhere in a label (« Payer en 1 x », « Pay in »): no item's name.
static PAY_ANY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:pay|payer|paiement|payment|bezahlen|zahlung|pagar|pago|betalen|betaling|r[ée]gler|fois|times|mal)\b")
        .expect("pay any pattern")
});
/// Cents on the line under their euros, with no decimal mark (« 32⏎05 € »): joined, read at their largest.
static CENTS_NEXT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d)\n(\d{2})(\s*(?:€|eur))").expect("cents next line pattern"));
/// A price read alone, with its cents (« 27,97 », « 1 234,56 »): the only group a quantity before it may be left
/// out for (« Qté 2 27,97 € »).
static WHOLE_PRICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,3}(?:[ .,'\x{2019}]\d{3})*[.,]\d{2}$").expect("price pattern"));
/// A total's label.
static TOTAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)total|montant|amount|betrag|gesamt|summe|importe|totaal|bedrag|סה״כ|סה\x22כ|סכום|לתשלום").expect("total pattern")
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
/// The words of a line after its last AMOUNT (a number next to a currency mark): a label's own words, numbers that
/// are no amount kept (« Total pour 2 passagers : », « Total à payer le 20/11 : », Relecture 8).
fn after_last_amount(line: &str) -> &str {
    let mut cut = 0;
    for m in NUMBER.find_iter(line) {
        if mark_near(&line[..m.start()], "").is_some() || mark_near("", &line[m.end()..]).is_some() {
            cut = m.end();
        }
    }
    &line[cut..]
}

fn after_last_digit(line: &str) -> &str {
    match line.rfind(|c: char| c.is_ascii_digit()) {
        Some(at) => &line[at + 1..],
        None => line,
    }
}

/// The class of an amount whose line, up to it, is `line_before`, the lines above being `above` (latest last).
fn class_of(line_before: &str, above: &[&str], below: Option<&str>) -> Class {
    // Exemptions (part, mention, struck, suggestion) come only from the amount's own words, on its own line, right
    // before it (Relecture 7, w1: never from the line above); the line above may only name a total.
    // inside brackets: « (dont 4,66 € de TVA) »
    if line_before.matches('(').count() > line_before.matches(')').count() {
        return Class::Part;
    }
    let own = after_last_amount(line_before);
    // page_snapshot.rs's marks, put right before each number of the piece that carries them
    let marked = own.trim_end();
    if marked.ends_with("[barré]") || marked.ends_with("[suggestion]") {
        return Class::Mention;
    }
    // before tax, a subtotal: a part, whatever else its own label says (its brackets included: « (excl. VAT) »)
    let own_words = INCLUDED.replace_all(own, " ");
    if !mark_only(own) && PART_ALWAYS.is_match(last_chars(own_words.trim(), LABEL_SPAN)) && !is_mention(own.trim(), line_before) {
        return Class::Part;
    }
    let mut cleaned = line_before.to_string();
    while BRACKETS.is_match(&cleaned) {
        cleaned = BRACKETS.replace_all(&cleaned, " ").into_owned();
    }
    let label = after_last_amount(&cleaned).to_string();
    if mark_only(&label) {
        // the line above, when it holds no amount of its own (« Total à payer: » then « EUR 27.97 »): a total's name
        // only, never an exemption; with no line of words above, the line right below (« 32,87 € » then « Total TTC »)
        let above = above.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty() && !mark_only(l));
        let named = match above {
            Some(l) if !l.chars().any(|c| c.is_ascii_digit()) => l.to_string(),
            Some(_) => String::new(),
            None => below
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.chars().any(|c| c.is_ascii_digit()))
                .unwrap_or("")
                .to_string(),
        };
        let named = INCLUDED.replace_all(&named, " ");
        let tail = last_chars(named.trim(), LABEL_SPAN);
        if tail.contains('[') || PART_ALWAYS.is_match(tail) || PART.is_match(tail) {
            return Class::None;
        }
        return if TO_PAY.is_match(tail) || PAY_WORD.is_match(tail) || pay_heading(&named) {
            Class::ToPay
        } else if TOTAL.is_match(tail) {
            Class::Total
        } else {
            Class::None
        };
    }
    // « inkl. MwSt. », « TVA incluse », « livraison comprise »: what a total includes, never a part (Relecture 8)
    let label = INCLUDED.replace_all(&label, " ");
    let tail = last_chars(label.trim(), LABEL_SPAN);
    // a total to pay's words win over a part's (« Total TTC, livraison offerte : ») and a mention's; a mention only
    // when its words end the label, right before the amount
    // a total to pay's words always win over a mention's (« Te betalen saldo », Relecture 9): a word that can mean the
    // amount due never exempts
    if TO_PAY.is_match(tail) || PAY_WORD.is_match(tail) {
        Class::ToPay
    } else if is_mention(tail, line_before) {
        Class::Mention
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
    // (no digit is ever removed here, Relecture 8: the card being typed is taken out by page_snapshot.rs, and only it)
    let text = EURO_CENTS.replace_all(&text, "$1,$2 €$3");
    let text = LOOSE_CENTS.replace_all(&text, "$1$2");
    CENTS_NEXT_LINE.replace_all(&text, "$1 $2$3").into_owned()
}

/// A currency mark separated from its number by `gap`: on the same line, or alone on its own line (`text`: the text up
/// to the mark when it is before, from the mark when it is after).
fn mark_line_ok(text: &str, gap: &str, before: bool) -> bool {
    if !gap.contains('\n') {
        return true;
    }
    let line = if before { text.rsplit('\n').next().unwrap_or("") } else { text.split('\n').next().unwrap_or("") };
    mark_only(line)
}

/// The line after the one `at` is on, when the rest of that line holds no words (only marks).
fn next_line(text: &str, at: usize) -> Option<&str> {
    let rest = &text[at..];
    let end = rest.find('\n')?;
    if !mark_only(&rest[..end]) {
        return None;
    }
    let after = &rest[end + 1..];
    Some(after.split('\n').next().unwrap_or(""))
}

/// The codes of an amount whose currency is on another line, with words: never approved, never ignored.
pub const ORPHAN: &[&str] = &["~"];

/// An id's mark right before a number (« #10045 », « n° 1234 », « No. 55 », « Nr. 7 »).
static ID_BEFORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:#|\bn°|\bnº|\bno\.|\bnr\.?|\bnum[ée]ro|\bnummer)\s*$").expect("id pattern"));

/// A number's largest plausible reading in cents (Relecture 7): as written, else its groups joined, else all its digits as
/// whole units.
fn largest_reading(whole: &str) -> Option<i64> {
    if let Some(cents) = amount_of(whole) {
        return Some(cents);
    }
    let joined: String = whole.chars().filter(|c| !c.is_whitespace()).collect();
    amount_of(&joined).or_else(|| {
        let digits: Zeroizing<String> = Zeroizing::new(whole.chars().filter(char::is_ascii_digit).collect());
        digits.get(..digits.len().min(12)).and_then(|d| d.parse::<i64>().ok()).map(|u| u.saturating_mul(100))
    })
}

/// The currency mark next to a number (cards.rs `mark_near`), a bare « $ » made precise by the ISO code written right
/// against it (« CAD $32.87 », « 32,87 $ CAD », « AUD $ »: Relecture 9).
fn mark_of(before: &str, after: &str) -> Option<&'static [&'static str]> {
    let codes = mark_near(before, after)?;
    if codes.len() < 2 || !codes.contains(&"USD") {
        return Some(codes);
    }
    const DOLLARS: &[(&str, &[&str])] = &[
        ("usd", &["USD"]),
        ("cad", &["CAD"]),
        ("aud", &["AUD"]),
        ("sgd", &["SGD"]),
        ("nzd", &["NZD"]),
        ("hkd", &["HKD"]),
        ("mxn", &["MXN"]),
    ];
    let word_before = |t: &str| {
        let t = t.trim_end().strip_suffix('$')?.trim_end();
        let w = t.rsplit(|c: char| !c.is_alphabetic()).next()?;
        DOLLARS.iter().find(|(c, _)| *c == w).map(|(_, k)| *k)
    };
    let word_after = |t: &str| {
        let t = t.trim_start().strip_prefix('$')?.trim_start();
        let w = t.split(|c: char| !c.is_alphabetic()).next()?;
        DOLLARS.iter().find(|(c, _)| *c == w).map(|(_, k)| *k)
    };
    Some(if !before.is_empty() { word_before(before) } else { word_after(after) }.unwrap_or(codes))
}

/// Every amount next to a currency mark in a page's text, in order.
pub fn amounts(text: &str) -> Vec<Amount> {
    let text = normalised(text);
    let mut out = Vec::new();
    let mut quantities: Vec<(usize, i64)> = Vec::new();
    for found in NUMBER.find_iter(&text) {
        let (start, end) = (found.start(), found.end());
        let (before, after) = (&text[..start], &text[end..]);
        // a mark on another line counts only when it stands alone on its line (« Total » / « € » / « 27,97 »): a number
        // on the line under « … 27,97 € » (a reference, a year, a card group) is no amount of that euro sign
        // an id (« Invoice #10045 », « Commande n° 1234 », « No. 55 ») is never an amount: it carries no currency of its
        // own (Relecture 9)
        // only a lone number: one that runs into another group (« n° 1004 32,87 € ») may hide an amount, read whole
        if ID_BEFORE.is_match(last_chars(before, 12)) && !found.as_str().contains(|c: char| c.is_whitespace()) {
            continue;
        }
        let gap_before = &before[before.trim_end().len()..];
        let gap_after = &after[..after.len() - after.trim_start().len()];
        let raw_before = mark_of(before, "");
        let raw_after = mark_of("", after);
        let mark_before = raw_before.filter(|_| mark_line_ok(before.trim_end(), gap_before, true));
        let mark_after = raw_after.filter(|_| mark_line_ok(after.trim_start(), gap_after, false));
        let Some(codes) = mark_after.or(mark_before) else {
            // a currency on the next or previous line, with words (« Total à payer 1249 » then « € TTC », « Total TTC
            // en € » then « 1249 »): which number it goes with is not sure; never ignored, a doubt (Relecture 9, y3)
            // the mark's line holds no number of its own (a mark with words only: « € TTC », « Total TTC en € »); a
            // mark already beside its own number (« Crème 27,97 € » above a stepper's « 1 ») is that number's
            let own_rest = after.split('\n').next().unwrap_or("");
            let alone = !own_rest.chars().any(char::is_alphanumeric);
            let no_digit = |l: &str| !l.chars().any(|c| c.is_ascii_digit());
            let line_above = before.trim_end().rsplit('\n').next().unwrap_or("");
            let line_below = after.trim_start().split('\n').next().unwrap_or("");
            let orphan = (raw_after.is_some() && gap_after.contains('\n') && no_digit(line_below))
                || (raw_before.is_some() && gap_before.contains('\n') && alone && no_digit(line_above));
            if orphan {
                if let Some(cents) = largest_reading(found.as_str()) {
                    out.push(Amount { cents, codes: ORPHAN, class: Class::None, field: false });
                }
                continue;
            }
            // « Total à payer : 32,87 », the currency elsewhere (a column's head): a total's number all the same
            if BARE_MONEY.is_match(found.as_str()) {
                if let Some(cents) = amount_of(found.as_str()) {
                    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
                    let above: Vec<&str> =
                        text[..line_start].lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
                    let class = class_of(&text[line_start..start], &above, next_line(&text, end));
                    if class >= Class::Total {
                        out.push(Amount { cents, codes: BARE, class, field: false });
                    }
                }
            }
            continue;
        };
        let whole = found.as_str();
        // Read at its largest plausible value when it doesn't read as one number (Relecture 7: « 32 05 € », « 3 2 , 8 7 € »
        // are 3 205 € and 32,87 €, never 5 € or 7 €): only blocking can come of over-reading. The one exception, a
        // quantity before a whole price (« Qté 2 27,97 € »), keeps the price next to the mark when it reads alone with
        // its cents.
        // a quantity right before a whole price (« Crème × 1 249,00 € »): the price. Only inside an item's own line
        // (a product's name, then « × », on the same line), never across lines (a panel's close button « × »), never
        // after a total's or a payment's words (« Total × », « Payer en 1 x »): a low reading never wins otherwise
        // (Relecture 9, y2)
        let quantity = {
            let own_line = &before[before.rfind('\n').map_or(0, |i| i + 1)..];
            let b = own_line.trim_end();
            let item = b.strip_suffix('×').or_else(|| b.strip_suffix(" x")).map(str::trim_end);
            item.is_some_and(|name| {
                name.chars().any(char::is_alphabetic)
                    && !name.chars().any(|c| c.is_ascii_digit())
                    && !TOTAL.is_match(name)
                    && !TO_PAY.is_match(name)
                    && !PAY_ANY.is_match(name)
            })
        };
        let mut high = None; // the quantity's whole reading (« 1 249,00 »), kept unless the page's total agrees
        let cents = match amount_of(whole) {
            Some(cents) if !(quantity && whole.contains(' ')) => cents,
            _ if quantity && whole.find(' ').is_some_and(|at| WHOLE_PRICE.is_match(&whole[at + 1..])) => {
                match whole.find(' ').and_then(|at| amount_of(&whole[at + 1..])) {
                    Some(cents) => {
                        high = amount_of(whole);
                        cents
                    }
                    None => continue,
                }
            }
            Some(cents) => cents,
            None => {
                let next_to_mark = if mark_after.is_some() {
                    whole.rfind(' ').map(|at| &whole[at + 1..])
                } else {
                    whole.find(' ').map(|at| &whole[..at])
                };
                match next_to_mark.filter(|g| WHOLE_PRICE.is_match(g)).and_then(amount_of) {
                    Some(cents) => cents,
                    None => {
                        let joined: String = whole.chars().filter(|c| !c.is_whitespace()).collect();
                        let digits: String = whole.chars().filter(char::is_ascii_digit).collect();
                        match amount_of(&joined) {
                            Some(cents) => cents,
                            None => match digits.get(..digits.len().min(12)).and_then(|d| d.parse::<i64>().ok()) {
                                Some(units) => units.saturating_mul(100),
                                None => continue,
                            },
                        }
                    }
                }
            }
        };
        let number_start = start;
        let before = &text[..number_start];
        if PER_BEFORE.is_match(last_chars(before, 16)) || PER_AFTER.is_match(first_chars(after, 24)) {
            continue;
        }
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let above: Vec<&str> = text[..line_start].lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
        let line_before = &text[line_start..number_start];
        let field = after_last_digit(line_before).contains("[champ]");
        // a right-to-left line (Hebrew: « ₪32.87 סה״כ לתשלום » as drawn, left to right): its label is after the amount
        let own_line = &after[..after.find('\n').unwrap_or(after.len())];
        let rtl = |t: &str| t.chars().any(|c| ('\u{0590}'..='\u{05ff}').contains(&c));
        let rtl_label = (rtl(own_line) && mark_only(after_last_amount(line_before))).then(|| {
            let rest = NUMBER.find(own_line).map_or(own_line, |m| &own_line[..m.start()]);
            rest.trim_start_matches(|c: char| !c.is_alphabetic()).to_string()
        });
        let line_before = rtl_label.as_deref().unwrap_or(line_before);
        if let Some(high) = high {
            quantities.push((out.len(), high));
        }
        out.push(Amount { cents, codes, class: class_of(line_before, &above, next_line(&text, end)), field });
    }
    // a quantity's low reading stands only when a labelled total of the page is that price (« Crème × 1 249,00 € »
    // then « Total 249,00 € »); else the whole number, the higher reading (Relecture 9, y2: a low reading never wins
    // alone)
    for (at, high) in quantities {
        let low = out[at].cents;
        let agreed = out.iter().enumerate().any(|(i, a)| i != at && a.class >= Class::Total && a.cents == low);
        if !agreed {
            out[at].cents = high;
        }
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
    let orphan = |r: &Read| r.amount.codes == ORPHAN;
    let foreign: Vec<&Read> = all.iter().filter(|r| !marked(r) && !bare(r) && !orphan(r)).collect();
    let seen = || Seen {
        amounts: all_ours.iter().map(|r| r.amount.cents).collect(),
        labelled: all_ours.iter().filter(|r| r.amount.class >= Class::Total).count(),
        other_currency: foreign.len(),
    };
    let labelled: Vec<&Read> = shown_ours.iter().copied().filter(|r| r.amount.class >= Class::Total).collect();
    // 1. the page asks more: a visible total above the approved amount, or the provider's own visible amount above it
    // The amount is NAMED to the person (« la page demandait … ») only when it is beyond doubt what the page asks: the
    // one visible labelled total above, and the approved amount not itself labelled a total to pay on the page (a
    // gift card's « Reste à payer », a deposit's « À payer maintenant », Relecture 8); else a doubt, still a stop.
    let above: Vec<&Read> = labelled.iter().copied().filter(|r| r.amount.cents > cents).collect();
    let approved_to_pay = labelled.iter().any(|r| r.amount.cents == cents && r.amount.class == Class::ToPay);
    let mut above_values: Vec<i64> = above.iter().map(|r| r.amount.cents).collect();
    above_values.sort_unstable();
    above_values.dedup();
    if let Some(total) = pick(&above) {
        let providers_own = above.iter().any(|r| r.provider && r.amount.cents == total);
        if (above_values.len() == 1 && !approved_to_pay) || providers_own {
            return PageTotal::Other(total);
        }
        return PageTotal::Doubt(Doubt::Unclear);
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
    if let Some(higher) = visible_higher.chain(all_ours.iter().filter(blocks)).max_by_key(|r| r.amount.cents) {
        // a part above it (a subtotal before a discount) is not « a higher amount » the page asks: « could not check »
        if higher.amount.class == Class::Part {
            return PageTotal::Doubt(Doubt::Unclear);
        }
        return PageTotal::Doubt(Doubt::Higher(higher.amount.cents));
    }
    if all.iter().any(|r| bare(r) && r.amount.cents != cents && !uses_currency) {
        return PageTotal::Doubt(Doubt::Unclear);
    }
    // a visible number whose currency is on another line, with words: not sure which amount it is (Relecture 9, y3)
    if shown.iter().any(|r| orphan(r) && r.amount.cents != cents) {
        return PageTotal::Doubt(Doubt::Unclear);
    }
    // 3. the provider's own amounts and every total to pay must be the approved one
    let provider: Vec<i64> = shown_ours.iter().filter(|r| r.provider && marked(r)).map(|r| r.amount.cents).collect();
    let provider_agrees = provider.is_empty() || provider.contains(&cents);
    let to_pay_agree = shown_ours.iter().filter(|r| r.amount.class == Class::ToPay).all(|r| r.amount.cents == cents);
    if !provider_agrees || !to_pay_agree {
        return PageTotal::Doubt(Doubt::Unclear);
    }
    // 4. beyond doubt (and the whole page read): the approved amount, visible and labelled as the total (a total, a
    // total to pay, a pay button's amount), its currency written (Relecture 7: approval stays narrow; an unlabelled
    // largest amount is no longer enough)
    // `$` alone stands for USD, CAD, SGD, AUD…: it approves only a payment in USD; any other needs its own code (« CA$ »,
    // « CAD », « S$ ») (Relecture 8)
    let labelled_approved =
        labelled.iter().any(|r| r.amount.cents == cents && marked(r) && (r.amount.codes.len() == 1 || currency == "USD"));
    let whole = texts.iter().all(|t| !t.truncated);
    if whole && labelled_approved {
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
        let page = "Crème solaire SPF50 [barré] 35,00 € 27,97 €\nCoffret soin [suggestion] 49,90 €\nTotal à payer 27,97 €";
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
        // the whole page not read (page_snapshot.rs `MAX_CHARS`): never approved
        let cut = PageText { truncated: true, ..PageText::plain("Total à payer 27,97 €") };
        assert!(matches!(page_total_of(&[cut], 2797, "EUR"), PageTotal::Unread(_)));
        // a mention only when its words qualify the amount itself
        assert_eq!(total(&["Total 27,97 €\nLivraison offerte dès 49,00 €"], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total(&["Total panier 27,97 €\nTotal TTC, livraison offerte : 32,87 €"], 2797, "EUR"), PageTotal::Other(3287));
    }

    #[test]
    fn no_digit_is_ever_removed_here() {
        // Relecture 8: a long digit run next to a mark is read (at its worst); only the card being typed is taken out,
        // by page_snapshot.rs, which knows it
        assert_ne!(total(&["Total : €\n4242 4242 4242 4242"], 1250, "EUR"), PageTotal::Approved);
        assert_ne!(total(&["Total 1249 €"], 119_900, "EUR"), PageTotal::Approved);
        assert_eq!(total(&["Total 1249 €"], 124_900, "EUR"), PageTotal::Approved);
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

/// Relecture 7 (20/11, on 9b87714a5): each asserts the expected outcome; it fails on 9b87714a5.
#[cfg(test)]
mod review_7 {
    use super::*;

    #[test]
    fn w1_the_new_price_on_the_line_under_a_struck_old_one_is_not_a_mention() {
        // a promo total drawn on two rows (« ~~35,00 €~~ » then the big « 32,87 € »): the new price's own label is
        // empty, so class_of takes the line above as its raw label, finds « [barré] » there, and the real total is a
        // mention → ignored → « approved » on the basket line
        for page in [
            "Montant de votre panier : 27,97 €\nFrais de livraison 4,90 €\nTotal à payer\n[barré] 35,00 €\n32,87 €",
            // a right-floated amount first in the DOM (Bootstrap « pull-right »): « 32,87 € Total à payer », the row
            // above a struck delivery fee
            "Montant de votre panier : 27,97 €\n[barré] 4,90 € Offerte Livraison\n32,87 € Total à payer",
        ] {
            assert_ne!(page_total(&[page.to_string()], 2797, "EUR"), PageTotal::Approved, "{page}");
        }
    }
}

/// Relecture 8 (20/11, on bbc703ce4): `p*` assert the SAFE outcome and fail on bbc703ce4 (a page that asks more read
/// « approved »); `b*` measure good payments handed over (product side, not blockers).
#[cfg(test)]
mod review_8 {
    use super::*;

    fn total(page: &str, cents: i64, currency: &str) -> PageTotal {
        page_total(&[page.to_string()], cents, currency)
    }

    #[test]
    fn a_hosted_pages_pay_heading_names_its_amount() {
        // Stripe Checkout: « Pay <shop> » right above the amount
        assert_eq!(total("Pay Pharmacie Exemple\n€32.87\nOrdonnance\nPay with card", 3287, "EUR"), PageTotal::Approved);
        // « Pay later », « Pay with card », « Payer en 3 fois » name no amount to pay
        for heading in ["Pay later", "Pay with card", "Payer en plusieurs fois", "Pay monthly"] {
            assert_ne!(total(&format!("{heading}\n€32.87"), 3287, "EUR"), PageTotal::Approved, "{heading}");
        }
    }

    #[test]
    fn p1_a_line_starting_with_promo_right_after_the_total_makes_it_an_instalment() {
        // PER_AFTER: « pro » + « mo\b » matches « €⏎Promo code » (a coupon link right under the total): the real total is
        // dropped as « per month », the basket line reads « approved »
        for page in [
            "Montant de votre panier : 27,97 €\nFrais de livraison 4,90 €\nTotal à payer 32,87 €\nPromo code ?",
            "Total panier 27,97 €\nVersand 4,90 €\nGesamtsumme 32,87 €\nPromo-Code einlösen",
        ] {
            assert_ne!(total(page, 2797, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn b_labels_of_good_payments_measured() {
        // each page's total IS the approved amount: a good payment. Printed: which ones are handed over.
        let cases: &[(&str, i64, &str)] = &[
            ("Sous-total 27,97 €\nExpédition 4,90 €\nTotal EUR 32,87 €", 3287, "EUR"),
            ("Total à payer: EUR 27.97", 2797, "EUR"),
            ("Montant : 32,87 EUR", 3287, "EUR"),
            ("Montant à régler 214,00 €", 21400, "EUR"),
            ("Zwischensumme 27,97 €\nVersand 4,90 €\nGesamtsumme 32,87 €", 3287, "EUR"),
            ("Subtotal 27,97 €\nEnvío 4,90 €\nTotal a pagar 32,87 €", 3287, "EUR"),
            ("Subtotaal € 27,97\nVerzendkosten € 4,90\nTotaal € 32,87", 3287, "EUR"),
            ("Subtotal £27.97\nDelivery £4.90\nOrder total £32.87", 3287, "GBP"),
            ("Subtotal $27.97\nShipping $4.90\nTotal USD $32.87", 3287, "USD"),
            ("Total TTC 32,87 €", 3287, "EUR"),
            ("Reste à payer 32,87 €", 3287, "EUR"),
            ("Total de la commande 32,87 €", 3287, "EUR"),
            ("Prix total 32,87 €", 3287, "EUR"),
            ("Importe total 32,87 €", 3287, "EUR"),
            ("Te betalen € 32,87", 3287, "EUR"),
            ("Zu zahlender Betrag 32,87 €", 3287, "EUR"),
            ("Total due €32.87", 3287, "EUR"),
            ("Amount due £32.87", 3287, "GBP"),
            // labels expected to hand over (measured)
            ("Montant des articles 27,97 €\nFrais de port 4,90 €\nNet à payer 32,87 €", 3287, "EUR"),
            ("Zwischensumme 27,97 €\nVersandkosten 4,90 €\nGesamt inkl. MwSt. 32,87 €", 3287, "EUR"),
            ("Summe inkl. MwSt. 32,87 €", 3287, "EUR"),
            ("Total incl. VAT £32.87", 3287, "GBP"),
            ("Total TVA incluse 32,87 €", 3287, "EUR"),
            ("Total IVA incluido 32,87 €", 3287, "EUR"),
            ("Totaal incl. btw € 32,87", 3287, "EUR"),
            ("Total livraison comprise 32,87 €", 3287, "EUR"),
            ("Total pour 2 passagers : 64,00 €", 6400, "EUR"),
            ("Total for 2 nights €214.00", 21400, "EUR"),
            ("Total à payer le 20/11 : 32,87 €", 3287, "EUR"),
            ("Votre commande 32,87 €", 3287, "EUR"),
            ("Sous-total 35,00 €\nCode promo BIENVENUE −7,03 €\nTotal 27,97 €", 2797, "EUR"),
            ("Total articles 35,00 €\nRemise fidélité -7,03 €\nTotal à payer 27,97 €", 2797, "EUR"),
            ("Livraison offerte pour toute commande supérieure à 49 €\nTotal à payer 27,97 €", 2797, "EUR"),
            ("Free shipping on orders over €50\nOrder total €27.97", 2797, "EUR"),
            ("Cagnotte disponible : 50,00 €\nTotal à payer 27,97 €", 2797, "EUR"),
            ("Total 50,00 €\nCarte cadeau -22,03 €\nReste à payer 27,97 €", 2797, "EUR"),
            ("Prix total 214,00 €\nÀ payer maintenant 64,20 €\nÀ payer sur place 149,80 €", 6420, "EUR"),
            ("Crème solaire × 1 249,00 €\nSous-total 249,00 €\nTotal 249,00 €", 24900, "EUR"),
            ("Total 1249 €", 124900, "EUR"),
            ("Total CA$ 45.00", 4500, "CAD"),
        ];
        let mut handed = Vec::new();
        for (page, cents, currency) in cases {
            let got = total(page, *cents, currency);
            if got != PageTotal::Approved {
                handed.push(format!("{page:?} → {got:?}"));
            }
        }
        println!("handed over: {}/{}\n{}", handed.len(), cases.len(), handed.join("\n"));
    }
}

/// Relecture 9 (20/11, on 1fe82c895): each asserts the SAFE outcome (never « approved » when the page asks more); they
/// fail on 1fe82c895. All three come from rules added by Correction 9.
#[cfg(test)]
mod review_9 {
    use super::*;

    fn total(page: &str, cents: i64, currency: &str) -> PageTotal {
        page_total(&[page.to_string()], cents, currency)
    }

    #[test]
    fn y1_a_balance_to_pay_labelled_saldo_is_read_as_a_mention() {
        // MENTION's new « saldo » (meant for « saldo disponible »): in Dutch, German and Spanish « (te betalen / openstaand
        // / offener / rest) saldo » is the amount DUE. MENTION is checked before TO_PAY, so even « Te betalen saldo » is a
        // mention → the higher real total is ignored → « approved » on the basket's « Totaal » → pressed
        for page in [
            "Totaal € 27,97\nTransactiekosten € 0,50\nTe betalen saldo: € 28,47",
            "Totaal € 27,97\nServicekosten € 0,50\nRestsaldo € 28,47",
            "Gesamtsumme 27,97 €\nBearbeitungsgebühr 0,50 €\nOffener Saldo: 28,47 €",
            "Total 27,97 €\nGastos de gestión 0,50 €\nSaldo 28,47 €",
        ] {
            assert_ne!(total(page, 2797, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn y2_a_multiplication_or_close_sign_before_a_thousands_total_reads_only_its_last_groups() {
        // the new quantity rule (« Crème × 1 249,00 € » = 1 × 249,00): any « × » / « x » ending the text before the
        // number, even on the line above (a bottom sheet's close button « × », « Total × », « Payer en 1 x »), makes
        // « 1 249,00 € » read 249,00 € — under the approved 1 199,00 € → ignored → « approved »
        for page in [
            "Montant de votre panier : 1 199,00 €\nLivraison 50,00 €\nRécapitulatif ×\n1 249,00 €\nTotal TTC",
            "Montant de votre panier : 1 199,00 €\nLivraison 50,00 €\nTotal ×\n1 249,00 €",
            "Montant de votre panier : 1 199,00 €\nLivraison 50,00 €\nPayer en 1 x 1 249,00 €",
        ] {
            assert_ne!(total(page, 119_900, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn y3_a_total_whose_currency_mark_shares_its_line_with_words_is_not_read() {
        // the new mark_line_ok: a mark on another line counts only when alone on it. « 1249 » then « € TTC » (the price
        // block's small « € TTC » under the big number), or « Total en € » then « 1 249 »: no amount at all (no cents,
        // so no bare money either) → « approved » on the basket line. bbc703ce4 read them (over-read, fail-safe).
        for (page, cents) in [
            ("Montant de votre panier : 1 199,00 €\nLivraison 50,00 €\nTotal à payer 1249\n€ TTC", 119_900),
            ("Montant de votre panier : 1 199,00 €\nLivraison 50,00 €\nTotal TTC en €\n1249", 119_900),
            ("Total panier 27,97 €\nLivraison 4,90 €\n32,87\n€ TTC", 2797),
        ] {
            assert_ne!(total(page, cents, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn m_markets_measured() {
        // good payments in the markets sold to (Canada, Australia, Israel): printed when handed over
        let cases: &[(&str, i64, &str)] = &[
            ("Subtotal $27.97\nShipping $4.90\nTotal CAD $32.87", 3287, "CAD"),
            ("Sous-total 27,97 $\nLivraison 4,90 $\nTotal 32,87 $ CAD", 3287, "CAD"),
            ("Subtotal $27.97\nShipping $4.90\nTotal AUD $32.87", 3287, "AUD"),
            ("סכום ביניים ₪27.97\nמשלוח ₪4.90\nסה״כ לתשלום ₪32.87", 3287, "ILS"),
            ("Subtotal S$27.97\nDelivery S$4.90\nTotal S$32.87", 3287, "SGD"),
        ];
        let handed: Vec<String> = cases
            .iter()
            .filter_map(|(p, c, cur)| Some(total(p, *c, cur)).filter(|t| *t != PageTotal::Approved).map(|t| format!("{p:?} → {t:?}")))
            .collect();
        println!("handed over: {}/{}\n{}", handed.len(), cases.len(), handed.join("\n"));
    }
}

/// Correction 10 (after Relecture 9): what the new rules name, and what they never exempt.
#[cfg(test)]
mod correction_10 {
    use super::*;

    fn total(page: &str, cents: i64, currency: &str) -> PageTotal {
        page_total(&[page.to_string()], cents, currency)
    }

    #[test]
    fn an_iso_code_against_the_dollar_sign_names_its_currency() {
        assert_eq!(total("Subtotal $27.97\nShipping $4.90\nTotal CAD $32.87", 3287, "CAD"), PageTotal::Approved);
        assert_eq!(total("Sous-total 27,97 $\nLivraison 4,90 $\nTotal 32,87 $ CAD", 3287, "CAD"), PageTotal::Approved);
        assert_eq!(total("Subtotal $27.97\nShipping $4.90\nTotal AUD $32.87", 3287, "AUD"), PageTotal::Approved);
        // another dollar named against the sign: another currency, never approved
        assert_ne!(total("Total USD $32.87", 3287, "CAD"), PageTotal::Approved);
        // « $ » alone still only for USD
        assert_ne!(total("Total $32.87", 3287, "CAD"), PageTotal::Approved);
    }

    #[test]
    fn hebrew_labels_in_both_orders() {
        let logical = "סכום ביניים ₪27.97\nמשלוח ₪4.90\nסה״כ לתשלום ₪32.87";
        assert_eq!(total(logical, 3287, "ILS"), PageTotal::Approved);
        // as drawn left to right on a right-to-left page: the label after the amount
        let drawn = "₪27.97 סכום ביניים\n₪4.90 משלוח\n₪32.87 סה״כ לתשלום";
        assert_eq!(total(drawn, 3287, "ILS"), PageTotal::Approved);
        // the subtotal is a part, never the total: a higher one still blocks
        assert_ne!(total("₪40.00 סה״כ לתשלום\n₪32.87 סכום ביניים", 3287, "ILS"), PageTotal::Approved);
    }

    #[test]
    fn an_id_is_no_amount_and_a_threshold_needs_free_delivery() {
        assert_eq!(total("Invoice #10045 $32.87\nBalance due $32.87", 3287, "USD"), PageTotal::Approved);
        assert_eq!(total("Commande n° 1004\nTotal à payer 32,87 €", 3287, "EUR"), PageTotal::Approved);
        // an id running into a price's groups is never dropped: read as one number, the price beside its mark
        // (Relecture 7's rule), and a higher one still blocks
        assert_eq!(total("Commande n° 1004 32,87 €\nTotal à payer 32,87 €", 3287, "EUR"), PageTotal::Approved);
        assert_ne!(total("Commande n° 1004 99,00 €\nTotal à payer 32,87 €", 3287, "EUR"), PageTotal::Approved);
        // a threshold's words without free delivery: no mention
        assert_ne!(total("Total 27,97 €\nTotal à payer, plus de 32,87 €", 2797, "EUR"), PageTotal::Approved);
        assert_ne!(total("Total 27,97 €\nMore than €32.87", 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total("Free shipping on orders over €50\nOrder total €27.97", 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn a_quantitys_low_reading_needs_the_total_to_agree() {
        assert_eq!(total("Crème solaire × 1 249,00 €\nTotal 249,00 €", 24_900, "EUR"), PageTotal::Approved);
        // nothing agrees with 249,00: the whole number, 1 249,00, blocks
        assert_ne!(total("Montant de votre panier : 1 199,00 €\nRécapitulatif × 1 249,00 €", 119_900, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn a_mark_beside_its_own_number_makes_no_orphan() {
        // a stepper's « 1 » under a priced line: that line's « € » is its price's
        assert_eq!(total("Crème 27,97 €\n1\nTotal à payer 27,97 €", 2797, "EUR"), PageTotal::Approved);
        // a mark with words only, on the line above or below: a doubt
        assert_eq!(total("Total 27,97 €\nTotal TTC en €\n1249", 2797, "EUR"), PageTotal::Doubt(Doubt::Unclear));
        assert_eq!(total("Total 27,97 €\nTotal à payer 1249\n€ TTC", 2797, "EUR"), PageTotal::Doubt(Doubt::Unclear));
    }
}

/// Relecture 10 (20/11, on f4d65a5bb): each asserts the SAFE outcome (never « approved » when the page asks more); they
/// fail on f4d65a5bb.
#[cfg(test)]
mod review_10 {
    use super::*;

    fn total(page: &str, cents: i64, currency: &str) -> PageTotal {
        page_total(&[page.to_string()], cents, currency)
    }

    const BASKET: &str = "Montant de votre panier : 1 199,00 €\nLivraison 50,00 €\n";

    #[test]
    fn n1_a_number_before_a_thousands_total_on_its_line_leaves_only_its_last_group() {
        // amounts(): when the whole number doesn't read (« 2 1 249,00 »), the group next to the mark is taken if it reads
        // alone with its cents (Relecture 7's « Qté 2 27,97 € » exception) — with the French thousands space that group
        // is « 249,00 »: a quantity column, a footnote digit on the label, cut a 1 249,00 € total to 249,00 €. A low
        // reading wins alone (the principle Correction 10 states for « × »), so « approved » on the basket → pressed.
        for page in [
            "Montant de votre panier : 1 199,00 €\nArticle Qté Montant\nTéléviseur 1 1 199,00 €\nLivraison 1 50,00 €\nTotal 2 1 249,00 €".to_string(),
            format!("{BASKET}Prix total1 1 249,00 €"),
            format!("{BASKET}Total 3 1 249,00 €"),
        ] {
            assert_ne!(total(&page, 119_900, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn n2_an_integer_total_whose_mark_line_has_a_digit_or_whose_own_line_has_words_is_not_read() {
        // Correction 10's orphan: only when the mark's line has no digit and the number's own line nothing else. The
        // small line under a big integer price often has one (« € TTC (TVA 20 %) », « € TTC ou 4x 312,25 € », « dont
        // 0,50 € d'éco-participation »), the label line above too (« (2 articles) »), the number a word or a star after
        // it: « 1249 » is then no amount at all (no cents: no bare money either) → « approved » on the basket
        for tail in [
            "Total à payer 1249\n€ TTC (TVA 20 %)",
            "Total à payer 1249\n€ TTC ou 4x 312,25 €",
            "Total à payer 1249\n€ TTC dont 0,50 € d'éco-participation",
            "Total TTC en € (2 articles)\n1249",
            "Total TTC en €\n1249 TTC",
            "Total à payer 1249*\n€ TTC",
        ] {
            let page = format!("{BASKET}{tail}");
            assert_ne!(total(&page, 119_900, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn n3_a_labelled_integer_total_without_its_mark_right_beside_it_is_not_read() {
        // « Total TTC (€) : 1249 », « Total à payer en € : 1 249 », « Total à payer : 1249 » (the € in a column's head):
        // labelled bare totals are read only with cents (BARE_MONEY, Correction 2) → nothing read → « approved »
        for tail in ["Total TTC (€) : 1249", "Total à payer en € : 1 249", "Total à payer : 1249"] {
            let page = format!("{BASKET}{tail}");
            assert_ne!(total(&page, 119_900, "EUR"), PageTotal::Approved, "{page}");
        }
    }

    #[test]
    fn n4_lesser_an_unlabelled_visible_amount_without_mark_and_an_id_mark_on_the_line_above() {
        // LOW: « 32,87 » written big without its mark (an icon font's €) under the parts: ignored; « Facture n° » ending
        // the line above a lone total: the total is taken for an id
        assert_ne!(total("Total panier 27,97 €\nLivraison 4,90 €\n32,87", 2797, "EUR"), PageTotal::Approved);
        assert_ne!(total(&format!("{BASKET}Total à payer\nFacture n°\n1249,00 €"), 119_900, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn g_regression_woocommerce_items_of_100_to_999_euros_with_shipping_are_said_higher() {
        // Correction 10's quantity agreement: « Téléviseur × 1 249,00 € » keeps 1 249,00 unless a labelled total is
        // 249,00 — never true once shipping or a second item is added → Doubt::Higher(1 249,00 €), the person told « la
        // page affichait un montant plus élevé (1 249,00 €) »: a FALSE sentence and a good payment handed over (on
        // 1fe82c895: approved). Asserts the good outcome.
        let woo = "Votre commande\nProduit Sous-total\nTéléviseur × 1 249,00 €\nSous-total 249,00 €\nExpédition Colissimo : 4,90 €\nTotal 253,90 €";
        assert_eq!(total(woo, 25_390, "EUR"), PageTotal::Approved);
        let two = "Votre commande\nProduit Sous-total\nCrème × 1 129,00 €\nGel × 1 120,00 €\nSous-total 249,00 €\nTotal 249,00 €";
        assert_eq!(total(two, 24_900, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn m_more_good_payments_measured() {
        // good payments, printed when handed over (a measure, never failing)
        let cases: &[(&str, i64, &str)] = &[
            ("Livraison offerte pour 50 € d'achat\nSous-total 27,97 €\nTotal à payer 27,97 €", 2797, "EUR"),
            ("Free shipping on orders €50+\nSubtotal €27.97\nOrder total €27.97", 2797, "EUR"),
            ("Spend €50 for free shipping\nSubtotal €27.97\nOrder total €27.97", 2797, "EUR"),
            ("Add a gift box €39.90 Add to order\nSubtotal €27.97\nOrder total €27.97", 2797, "EUR"),
            ("Sous-total 35,00 €\nVous économisez 7,03 €\nTotal à payer 27,97 €", 2797, "EUR"),
            ("EUR €\n2\nSous-total 27,97 €\nTotal à payer 27,97 €", 2797, "EUR"),
            ("Zwischensumme CHF 1’199.00\nVersand CHF 50.00\nTotal CHF 1’249.00", 124_900, "CHF"),
            ("Subtotaal € 1.199,-\nVerzendkosten € 50,-\nTotaal € 1.249,-", 124_900, "EUR"),
            ("סכום ביניים ₪27.97\nמשלוח ₪4.90\nסה״כ לתשלום ₪32.87\nכולל מע״מ ₪4.78", 3287, "ILS"),
            ("Total à payer 32,87 €\nOu 4 paiements de 8,22 € sans frais", 3287, "EUR"),
        ];
        let handed: Vec<String> = cases
            .iter()
            .filter_map(|(p, c, cur)| Some(total(p, *c, cur)).filter(|t| *t != PageTotal::Approved).map(|t| format!("{p:?} → {t:?}")))
            .collect();
        println!("handed over: {}/{}\n{}", handed.len(), cases.len(), handed.join("\n"));
    }
}
