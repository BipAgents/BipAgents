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
}

impl PageText {
    /// A text read as wholly visible, of the shop's page.
    pub fn plain(text: &str) -> PageText {
        PageText { shown: text.to_string(), all: text.to_string(), provider: false }
    }
}

/// What the page says of the payment's total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTotal {
    /// The page's total is the amount the person approved, beyond doubt.
    Approved,
    /// The page's total, in the payment's currency, is another amount (cents).
    Other(i64),
    /// No total the bridge can read for sure: what it saw, for the log.
    Unread(Seen),
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
/// A digit run that may be a card number (12 to 19 digits, spaces or dashes between): never read.
static CARD_LIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d(?:[ -]?\d){11,18}").expect("card pattern"));
/// « 27€97 » (the cents right after the mark; `pageText` glues « 27€<sup>97</sup> » so).
static EURO_CENTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d)€(\d{2})([^\d.,]|$)").expect("euro cents pattern"));
/// « 27 ,97 » (the cents in a piece of their own).
static LOOSE_CENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d) ([.,]\d{2})\b").expect("cents pattern"));
/// A bracketed piece of a line (« (dont 4,66 € de TVA) »).
static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\([^()]*\)").expect("brackets pattern"));
/// An instalment right before the amount (« 3 x 9,32 € », « 3 fois 9,32 € », « 4x € 9,32 »).
static PER_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\d\s*(?:[x×]|fois|times|mal|veces|cuotas|termijnen|keer)\s*(?:€|eur|\$|£)?\s*$")
        .expect("instalment pattern")
});
/// An instalment right after it (« 9,32 € / mois », « 9,32 € par mois », « $9.32/mo »).
static PER_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:€|eur|euros?|\$|£|usd|gbp|chf)?\s*(?:/|par|per|pro|al|por|a|each|chacun)\s*(?:mois|month|mo\b|monat|mes|maand|semaine|week)")
        .expect("instalment pattern")
});
/// A label of an amount never charged: a threshold, a limit, an old price.
static MENTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bd[èe]s\b|[àa] partir d|\bfrom\b|\bab\b|\bdesde\b|\bvanaf\b|\bmax|plafond|\blimit|jusqu|up to|bis zu|\bhasta\b|au lieu d|instead of|\bstatt\b|en lugar d|in plaats van|\bavant\b|\bwas\b|\bantes\b|offert|gratuit|\bfree\b|gratis|kostenlos",
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
    if MENTION.is_match(tail) {
        Class::Mention
    } else if PART.is_match(tail) {
        Class::Part
    } else if TO_PAY.is_match(tail) {
        Class::ToPay
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
        let Some(codes) = mark_after.or(mark_before) else { continue };
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
        out.push(Amount { cents, codes, class: class_of(&text[line_start..number_start], &above) });
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
}

fn read_all(texts: &[PageText], shown: bool) -> Vec<Read> {
    texts
        .iter()
        .flat_map(|t| {
            amounts(if shown { &t.shown } else { &t.all }).into_iter().map(|amount| Read { amount, provider: t.provider })
        })
        .collect()
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
    let ours = |r: &&Read| r.amount.codes.contains(&currency);
    let shown_ours: Vec<&Read> = shown.iter().filter(ours).collect();
    let all_ours: Vec<&Read> = all.iter().filter(ours).collect();
    let foreign: Vec<&Read> = all.iter().filter(|r| !ours(r)).collect();
    let seen = || Seen {
        amounts: all_ours.iter().map(|r| r.amount.cents).collect(),
        labelled: all_ours.iter().filter(|r| r.amount.class >= Class::Total).count(),
        other_currency: foreign.len(),
    };
    let labelled: Vec<&Read> = shown_ours.iter().copied().filter(|r| r.amount.class >= Class::Total).collect();
    // 1. a visible total above the approved amount: the page asks more
    let above: Vec<&Read> = labelled.iter().copied().filter(|r| r.amount.cents > cents).collect();
    if let Some(total) = pick(&above) {
        return PageTotal::Other(total);
    }
    // 2. doubt: any amount above it laid out (off-screen included), or a total in another currency (DCC)
    let higher = all_ours.iter().any(|r| r.amount.cents > cents && r.amount.class != Class::Mention);
    let dcc = foreign.iter().any(|r| r.amount.class != Class::Mention);
    if higher || dcc {
        return PageTotal::Unread(seen());
    }
    // 3. beyond doubt: the provider's own amounts and every total to pay agree, the approved amount is the total
    let provider: Vec<i64> = shown_ours.iter().filter(|r| r.provider).map(|r| r.amount.cents).collect();
    let provider_agrees = provider.is_empty() || (provider.iter().max() == Some(&cents) && provider.contains(&cents));
    let to_pay_agree = shown_ours.iter().filter(|r| r.amount.class == Class::ToPay).all(|r| r.amount.cents == cents);
    let labelled_approved = labelled.iter().any(|r| r.amount.cents == cents);
    let largest_unlabelled = labelled.is_empty()
        && shown_ours.iter().any(|r| r.amount.cents == cents)
        && shown_ours.iter().all(|r| r.amount.cents <= cents);
    if provider_agrees && to_pay_agree && (labelled_approved || largest_unlabelled) {
        return PageTotal::Approved;
    }
    // 4. the approved amount nowhere, one clear visible total (below it)
    if !all_ours.iter().any(|r| r.amount.cents == cents) {
        let mut values: Vec<i64> = labelled.iter().map(|r| r.amount.cents).collect();
        values.sort_unstable();
        values.dedup();
        if values.len() == 1 {
            return PageTotal::Other(values[0]);
        }
        let to_pay: Vec<&Read> = labelled.iter().copied().filter(|r| r.amount.class == Class::ToPay).collect();
        let mut pay_values: Vec<i64> = to_pay.iter().map(|r| r.amount.cents).collect();
        pay_values.sort_unstable();
        pay_values.dedup();
        if let [total] = pay_values.as_slice() {
            return PageTotal::Other(*total);
        }
    }
    PageTotal::Unread(seen())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(texts: &[&str], cents: i64, currency: &str) -> PageTotal {
        page_total(&texts.iter().map(|t| t.to_string()).collect::<Vec<_>>(), cents, currency)
    }

    /// The page of 19/11 (pharmaciedesdrakkars.com → payment.cawl-solutions.fr), as `pageText` reads it.
    const CAWL: &str = "Pharmacie des Drakkars\nRécapitulatif de la commande\nTotal à payer:\nEUR 27.97\nNuméro de \
                        carte\nDate d'expiration\nCryptogramme visuel\nPayer\nAnnuler";

    #[test]
    fn the_cawl_page_of_19_11_shows_the_approved_total() {
        assert_eq!(total(&[CAWL], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total(&["Total à payer: EUR 27.97"], 2797, "EUR"), PageTotal::Approved);
        // another amount approved: the page's total said
        assert_eq!(total(&[CAWL], 2450, "EUR"), PageTotal::Other(2797));
    }

    #[test]
    fn every_common_way_of_writing_an_amount_is_read() {
        for written in ["27,97 €", "27.97 €", "EUR 27.97", "27.97 EUR", "€ 27,97", "€27.97", "27,97 EUR", "27,97\u{a0}€",
                        "27,97\u{202f}€", "27,97\u{2009}€", "EUR\u{a0}27,97", "27,97 euros", "27,97\n€", "€\n27,97",
                        "27,97 Euro"]
        {
            assert_eq!(total(&[&format!("Total\n{written}")], 2797, "EUR"), PageTotal::Approved, "{written:?}");
            assert_eq!(total(&[&format!("Total\n{written}")], 2798, "EUR"), PageTotal::Other(2797), "{written:?}");
        }
        for written in ["1 234,56 €", "1\u{202f}234,56 €", "1\u{a0}234,56 €", "1.234,56 €", "€1,234.56", "EUR 1234.56",
                        "1'234.56 EUR"]
        {
            assert_eq!(total(&[&format!("Montant total : {written}")], 123_456, "EUR"), PageTotal::Approved, "{written:?}");
        }
    }

    #[test]
    fn providers_summaries_are_read() {
        let pages = [
            // Stripe Checkout
            "Payer Pharmacie des Drakkars\n€27.97\nSubtotal\n€24.50\nShipping\n€3.47\nTotal due\n€27.97\nCard information\nPay",
            // Adyen drop-in on the shop's page
            "Order summary\nItems (2) 24,50 €\nDelivery 3,47 €\nTotal 27,97 €\nCard number\nExpiry date\nPay 27,97 €",
            // PayPlug
            "Montant à régler\n27,97 €\nPharmacie des Drakkars\nNuméro de carte\nPayer",
            // Lyra / PayZen
            "Boutique : Pharmacie des Drakkars\nMontant : 27,97 EUR\nRéférence commande : 100045871\nVISA\nMASTERCARD",
            // Monetico
            "Paiement sécurisé\nMontant : 27,97 EUR\nDate : 19/11/2026\nRéférence : CMD45871\nNuméro de carte",
            // Up2Pay e-Transactions
            "MONTANT 27,97 EUR\nCOMMERCANT PHARMACIE DES DRAKKARS\nREFERENCE 45871\nVALIDER",
            // German, Spanish, Dutch shops
            "Zwischensumme 24,50 €\nVersand 3,47 €\nGesamtsumme 27,97 €",
            "Subtotal 24,50 €\nEnvío 3,47 €\nTotal a pagar 27,97 €",
            "Subtotaal € 24,50\nVerzendkosten € 3,47\nTotaal € 27,97",
        ];
        for page in pages {
            assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Approved, "{page}");
            // the subtotal approved, the shipping forgotten: the page's total said
            assert_eq!(total(&[page], 2450, "EUR"), PageTotal::Other(2797), "{page}");
        }
    }

    #[test]
    fn a_part_of_the_total_is_never_the_total() {
        // subtotal, shipping, a discount
        // (an amount above the approved one, even a part: not beyond doubt, the agent's reading as before)
        let page = "Total articles 30,00 €\nRemise -5,00 €\nLivraison offerte\nTotal 25,00 €";
        assert!(matches!(total(&[page], 2500, "EUR"), PageTotal::Unread(_)));
        assert_ne!(total(&[page], 3000, "EUR"), PageTotal::Approved);
        // the shipping forgotten: the page's higher total said
        let page = "Total panier 27,97 €\nFrais de livraison 4,90 €\nTotal 32,87 €";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Other(3287));
        // a page that only mentions amounts: never a reason to cancel
        let mention = "Livraison offerte dès 50,00 €\nNuméro de carte\nPayer";
        assert!(matches!(total(&[mention], 2797, "EUR"), PageTotal::Unread(_)));
    }

    #[test]
    fn instalments_are_left_out() {
        let page = "Total 27,97 €\nou payez en 3 x 9,32 €\nou 3 fois 9,33 € sans frais\nsoit 9,32 € / mois";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total(&[page], 932, "EUR"), PageTotal::Other(2797));
    }

    #[test]
    fn a_quantity_before_the_amount_is_not_part_of_it() {
        let page = "Doliprane 1000 mg\nQté 2 27,97 €";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn the_card_frame_and_the_page_are_read_together() {
        // the total in the shop's page, the card's fields in the provider's frame
        let parent = "Votre commande\nTotal TTC 27,97 €";
        let frame = "Numéro de carte\nMM / AA\nCVC";
        assert_eq!(total(&[parent, frame], 2797, "EUR"), PageTotal::Approved);
        assert_eq!(total(&[frame, parent], 2450, "EUR"), PageTotal::Other(2797));
        // the total in the provider's frame
        assert_eq!(total(&["Panier", "Montant : 27,97 EUR\nNuméro de carte"], 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn no_total_readable_keeps_the_agents_word() {
        // no amount at all; a card number, an expiry, a code as a page could show them: never amounts
        for page in ["Numéro de carte\nDate d'expiration\nPayer", "4242 4242 4242 4242\n12/30\n123", ""] {
            assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Unread(Seen::default()), "{page}");
        }
        // only another currency: not this payment's total
        let usd = total(&["Total USD 30.12"], 2797, "EUR");
        assert_eq!(usd, PageTotal::Unread(Seen { amounts: vec![], labelled: 0, other_currency: 1 }));
        // several amounts, no label, the approved one not the largest: not read for sure
        assert!(matches!(total(&["Doliprane 3,50 €\nSpray 24,47 €\nCrème 30,00 €"], 2447, "EUR"), PageTotal::Unread(_)));
        // the same, the approved one the largest (the total, unlabelled): approved
        assert_eq!(total(&["Doliprane 3,50 €\nSpray 24,47 €\n27,97 €"], 2797, "EUR"), PageTotal::Approved);
    }

    #[test]
    fn an_unlabelled_grand_total_above_labelled_parts_is_approved() {
        let page = "Montant des articles 24,50 €\nFrais de port 3,47 €\n27,97 €";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Approved);
        // a total labelled lower, the approved one unlabelled under it: not beyond doubt
        let page = "Montant 24,50 €\nFrais de port 3,47 €\n27,97 €";
        assert!(matches!(total(&[page], 2797, "EUR"), PageTotal::Unread(_)));
        assert_eq!(total(&[page], 2000, "EUR"), PageTotal::Other(2450));
    }

    #[test]
    fn dollars_pounds_and_swiss_francs() {
        assert_eq!(total(&["Order total: $34.90"], 3490, "USD"), PageTotal::Approved);
        assert_eq!(total(&["Order total: $34.90"], 3000, "USD"), PageTotal::Other(3490));
        assert_eq!(total(&["Total to pay £12.00"], 1200, "GBP"), PageTotal::Approved);
        assert_eq!(total(&["Total CHF 1’250.00"], 125_000, "CHF"), PageTotal::Approved);
    }
}

/// Relecture 1 (19/11): pages that prove a wrong « approved » (the worst case: the agent is told to confirm) or a wrong
/// « other total » (a good payment cancelled, the person told a false amount). Each asserts the SAFE outcome; they
/// fail on c56eda1cd.
/// What the press does once the page's total is read again right before it (card_pay.rs `confirm`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recheck {
    Press,
    /// Another total now: given up, the person told with the numbers.
    Other(i64),
    /// Not what it was when the card was typed (no longer approved, an amount above the approved one or another currency
    /// appeared): given up.
    Changed,
}

/// `before`: the total read before the card was typed; `now`: read again right before the press.
pub fn recheck(before: &PageTotal, now: &PageTotal, cents: i64) -> Recheck {
    match (before, now) {
        (_, PageTotal::Other(shown)) => Recheck::Other(*shown),
        (PageTotal::Approved, PageTotal::Approved) => Recheck::Press,
        (PageTotal::Approved, PageTotal::Unread(_)) => Recheck::Changed,
        (PageTotal::Unread(was), PageTotal::Unread(is)) => {
            let new_above = is.amounts.iter().any(|c| *c > cents && !was.amounts.contains(c));
            if new_above || is.other_currency > was.other_currency {
                Recheck::Changed
            } else {
                Recheck::Press
            }
        }
        _ => Recheck::Press,
    }
}

/// Correction 1 (19/11): what is visible, the provider's own amounts, other currencies, mentions.
#[cfg(test)]
mod correction_1 {
    use super::*;

    fn text(shown: &str, all: &str, provider: bool) -> PageText {
        PageText { shown: shown.into(), all: all.into(), provider }
    }

    #[test]
    fn only_visible_text_supports_approved_and_any_laid_out_amount_above_blocks_it() {
        // the visible total is higher; an off-screen « to pay » line says the approved amount
        let t = text("Total 35,00 €", "Total 35,00 €\nTotal à payer 27,97 €", false);
        assert_eq!(page_total_of(&[t], 2797, "EUR"), PageTotal::Other(3500));
        // the visible total is the approved one; an off-screen higher total: never approved
        let t = text("Total à payer 27,97 €", "Total à payer 27,97 €\nTotal 35,00 €", false);
        assert!(matches!(page_total_of(&[t], 2797, "EUR"), PageTotal::Unread(_)));
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
        assert!(matches!(page_total_of(&[shop, fee], 2797, "EUR"), PageTotal::Unread(_)));
    }

    #[test]
    fn a_total_in_another_currency_is_never_approved() {
        let page = "Total à payer 27,97 €\nMontant débité : USD 32.10";
        assert!(matches!(total(&[page], 2797, "EUR"), PageTotal::Unread(_)));
        let t = text("Total à payer 27,97 €", "Total à payer 27,97 €", false);
        let provider = text("32,10 USD", "32,10 USD", true);
        assert!(matches!(page_total_of(&[t, provider], 2797, "EUR"), PageTotal::Unread(_)));
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
        let unread = |amounts: Vec<i64>, other_currency: usize| {
            PageTotal::Unread(Seen { amounts, labelled: 0, other_currency })
        };
        assert_eq!(recheck(&PageTotal::Approved, &PageTotal::Approved, 2797), Recheck::Press);
        assert_eq!(recheck(&PageTotal::Approved, &PageTotal::Other(2947), 2797), Recheck::Other(2947));
        assert_eq!(recheck(&PageTotal::Approved, &unread(vec![2797, 2947], 0), 2797), Recheck::Changed);
        assert_eq!(recheck(&unread(vec![], 0), &PageTotal::Approved, 2797), Recheck::Press);
        assert_eq!(recheck(&unread(vec![500], 0), &unread(vec![500], 0), 2797), Recheck::Press);
        assert_eq!(recheck(&unread(vec![500], 0), &unread(vec![500, 3000], 0), 2797), Recheck::Changed);
        assert_eq!(recheck(&unread(vec![], 0), &unread(vec![], 1), 2797), Recheck::Changed);
        // the fee of Relecture 1 (Chrome r3): « Total à payer: EUR 27.97 » becomes « EUR 29.47 » after the number
        let before = page_total(&["Total à payer:\nEUR 27.97".to_string()], 2797, "EUR");
        let now = page_total(&["Frais carte : EUR 1.50\nTotal à payer:\nEUR 29.47".to_string()], 2797, "EUR");
        assert_eq!(recheck(&before, &now, 2797), Recheck::Other(2947));
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
        let PageTotal::Unread(seen) = total(&["27,97 €\n4242 4242 4242 4242\n12/30"], 1250, "EUR") else {
            panic!("expected unread")
        };
        assert!(!seen.amounts.contains(&424_200), "the card's first four digits in the log line: {seen:?}");
    }

    #[test]
    fn r8_a_misread_total_approved_again_is_never_the_page_s_total() {
        // total_differs tells the agent to offer the misread amount; approved again, it is then « the page's total »
        // and confirm presses « Payer », which pays the real 27,97 €
        assert_ne!(total(&["Total TTC (dont 4,66 € de TVA) 27,97 €"], 466, "EUR"), PageTotal::Approved);
        assert_ne!(total(&["Total TTC 27€97"], 2700, "EUR"), PageTotal::Approved);
    }
}
