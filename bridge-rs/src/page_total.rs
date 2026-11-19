//! The total a payment page shows, read by the bridge itself (fix-pay-total-check, 19/11): once the card is typed,
//! card_pay.rs reads the visible text of the page and of its card frames (card_fill.rs `Page::page_texts`, never a
//! field's value) and compares the total with the amount the person approved, so that the agent never has to compare
//! numbers itself (Vie on a CAWL page, 19/11: « Total à payer: EUR 27.97 », 27.97 approved, and the agent cancelled).
//!
//! How the total is found, in each text:
//! - every number next to a currency mark (cards.rs `mark_near`: « € », « EUR », « euros », « $ »…, before or after it,
//!   spaces of any width between) read as shops write it (cards.rs `amount_of`: « 27,97 », « 27.97 », « 1 234,56 »,
//!   « 1.234,56 », « 1,234.56 »); a number glued to a quantity (« Qté 2 27,97 € ») is read from its last group;
//! - an instalment (« 3 x 9,32 € », « 3 fois 9,32 € », « 9,32 € / mois ») is left out;
//! - each amount gets the label on the last line written between the amount before it and itself (its currency
//!   mark left out, 40 characters at most): a total's
//!   (« Total », « Montant », « Amount », « Betrag », « Importe », « Totaal »…), a total to pay's (« Total à payer »,
//!   « Amount due », « Zu zahlen », « Te betalen »…), or none for a part of it (« Sous-total », « Livraison »,
//!   « Remise », « HT », « Total articles »…).
//!
//! Then, with the amounts in the payment's currency only:
//! 1. one value labelled « to pay »: the total, approved or not;
//! 2. else the approved amount labelled as a total: approved;
//! 3. else amounts labelled as a total, the approved one not among them: not approved, unless the approved amount
//!    is on the page, unlabelled (never a part), above all of them;
//! 4. no labelled total: approved when the approved amount is the largest on the page; else not read.
//!
//! « Not approved » (`Other`) is only ever said from a labelled total: an amount the page only mentions (« Livraison
//! offerte dès 50 € ») never makes the bridge cancel. No amount in the payment's currency, or no clear total: `Unread`,
//! and the agent is told as before.

use std::sync::LazyLock;

use regex::Regex;

use crate::cards::{amount_of, mark_near};

/// What the page says of the payment's total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTotal {
    /// The page's total is the amount the person approved.
    Approved,
    /// The page's total, in the payment's currency, is another amount (cents).
    Other(i64),
    /// No total the bridge can read for sure: what it saw, for the log.
    Unread(Seen),
}

/// What was seen on a page whose total could not be read (logged, never the text: no card data, no address).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// Amounts in the payment's currency (cents, in order), and how many were labelled as a total.
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
    pub label: Label,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Label {
    /// A part of the total, or an amount only mentioned (« Sous-total », « Livraison », « dès 50 € »).
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
/// A label that says the amount is a part of the total, not the total.
static PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)sous[- ]?total|sub[- ]?total|zwischensumme|subtotaal|articles|produits|\bitems?\b|artikel|productos|hors taxe|\bht\b|\btva\b|\bvat\b|mwst|\biva\b|\bbtw\b|livraison|shipping|delivery|versand|env[ií]o|verzend|remise|discount|[ée]conomi|saving|rabatt|descuento|korting|r[ée]duction|frais|\bfees?\b|gebühr|cargo|kosten|d[èe]s\b|from\b|ab\b|desde|vanaf|offert|gratuit|free",
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

/// How far before an amount its label is looked for (characters, after the amount before it).
const LABEL_SPAN: usize = 40;

/// The label of an amount: the last line of `before` (the text since the amount before it) that is not only its
/// currency mark, its last `LABEL_SPAN` characters.
fn label_of(before: &str) -> Label {
    let mark_only = |line: &str| {
        line.chars().all(|c| !c.is_alphanumeric()) || (line.chars().count() <= 4 && mark_near(line, "").is_some())
    };
    let line = before.split('\n').map(str::trim).rfind(|l| !l.is_empty() && !mark_only(l)).unwrap_or("");
    let tail = last_chars(line, LABEL_SPAN);
    if PART.is_match(tail) {
        Label::Part
    } else if TO_PAY.is_match(tail) {
        Label::ToPay
    } else if TOTAL.is_match(tail) {
        Label::Total
    } else {
        Label::None
    }
}

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

/// Every amount next to a currency mark in a page's text, in order.
pub fn amounts(text: &str) -> Vec<Amount> {
    // one kind of space: thin, narrow, non-breaking and figure spaces as a plain one
    let text: String = text
        .to_lowercase()
        .chars()
        .map(|c| if matches!(c, '\u{a0}' | '\u{202f}' | '\u{2009}' | '\u{2007}' | '\u{200a}') { ' ' } else { c })
        .collect();
    let mut out = Vec::new();
    let mut previous_end = 0;
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
            previous_end = end;
            continue;
        }
        let label = label_of(&text[previous_end.min(number_start)..number_start]);
        previous_end = end;
        out.push(Amount { cents, codes, label });
    }
    out
}

/// What the page's texts (its own and its card frames', card_fill.rs `Page::page_texts`) say of the total of a payment
/// of `cents` in `currency` (« EUR »).
pub fn page_total(texts: &[String], cents: i64, currency: &str) -> PageTotal {
    let all: Vec<Amount> = texts.iter().flat_map(|t| amounts(t)).collect();
    let (same, other): (Vec<&Amount>, Vec<&Amount>) = all.iter().partition(|a| a.codes.contains(&currency));
    let seen = || Seen {
        amounts: same.iter().map(|a| a.cents).collect(),
        labelled: same.iter().filter(|a| a.label >= Label::Total).count(),
        other_currency: other.len(),
    };
    if same.is_empty() {
        return PageTotal::Unread(seen());
    }
    let values = |label: Label| {
        let mut v: Vec<i64> = same.iter().filter(|a| a.label >= label).map(|a| a.cents).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let to_pay: Vec<i64> = same.iter().filter(|a| a.label == Label::ToPay).map(|a| a.cents).collect();
    let mut to_pay_values = to_pay.clone();
    to_pay_values.sort_unstable();
    to_pay_values.dedup();
    if let [total] = to_pay_values.as_slice() {
        return if *total == cents { PageTotal::Approved } else { PageTotal::Other(*total) };
    }
    let totals = values(Label::Total);
    if totals.contains(&cents) {
        return PageTotal::Approved;
    }
    let on_page = same.iter().any(|a| a.cents == cents);
    if let Some(&largest) = totals.last() {
        // an unlabelled grand total above the labelled ones (never a part: « Total articles 30,00 € »)
        if same.iter().any(|a| a.cents == cents && a.label == Label::None) && cents > largest {
            return PageTotal::Approved;
        }
        return PageTotal::Other(largest);
    }
    let largest = same.iter().map(|a| a.cents).max().unwrap_or(0);
    if on_page && cents == largest {
        return PageTotal::Approved;
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
        let page = "Total articles 30,00 €\nRemise -5,00 €\nLivraison offerte\nTotal 25,00 €";
        assert_eq!(total(&[page], 2500, "EUR"), PageTotal::Approved);
        assert_eq!(total(&[page], 3000, "EUR"), PageTotal::Other(2500));
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
        // a total labelled lower, the approved grand total unlabelled under it
        let page = "Montant 24,50 €\nFrais de port 3,47 €\n27,97 €";
        assert_eq!(total(&[page], 2797, "EUR"), PageTotal::Approved);
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
