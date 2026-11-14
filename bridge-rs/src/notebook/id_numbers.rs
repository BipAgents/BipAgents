//! What the Carnet never keeps: ID numbers (passports, ID cards, social security, tax numbers, driving licences, bank
//! cards, IBAN), in the usual forms of the four languages (`lexicon::id_number`). An e-mail address or a site's login
//! is not one, nor a date, a time, a phone number, a book's ISBN, an order or a parcel's number.
//!
//! bridge-tools-no-breaker (14/11): on the sign-up bench, DeepSeek's note « Compte client créé le 14 novembre 2026 sur
//! la Pharmacie des Essais… » was refused three times in a row: the IBAN check squeezed the spaces out of the whole
//! text and read « LE14NOVEMBRE2026SURLA… » as one (two letters, two digits, eleven more). The agent took its e-mail
//! login for the culprit, took it out, and was refused again; Hermes then paused all the bridge's tools for a minute.
//! Now an IBAN is a country's code with its own length, written in one block or in groups of four, mostly digits;
//! e-mail and web addresses, dates, times, phone numbers (after their word) and ISBNs are set aside before any check;
//! short words that are also common words (« nie »: German for « never ») count only in capitals.
//!
//! Review 1 (14/11): the everyday names (« Steuer-Nr. », « SV-Nummer », « Pass-Nr. », « ID number », « pièce
//! d'identité »…), a NIR written with dots, a card with dashes or double spaces are refused; a long number with dashes
//! (an order, a book) is no card unless it has a card's shape; a parcel's or an order's long number is kept unless it
//! is a card's (its check digit, or a card named).

use std::sync::LazyLock;

use regex::Regex;

use super::fold;

/// Words that say an ID number follows, folded (`fold`); `*`: any word starting so (the German compounds).
const KEYS: &[&str] = &[
    // fr
    "passeport", "carte d identite", "carte identite", "carte nationale d identite", "piece d identite",
    "numero d identite", "titre de sejour", "permis de conduire", "securite sociale", "secu", "carte vitale",
    "numero fiscal", "carte bancaire", "numero de carte", "cni", "nir",
    // en
    "passport", "id card", "identity card", "id number", "id no", "driving licence", "driving license",
    "driver s license", "driver s licence", "drivers license", "driver license", "driver licence", "social security",
    "ssn", "national insurance", "ni number", "card number", "credit card", "tax id",
    // de
    "reisepass*", "pass nr", "passnummer", "personalausweis*", "ausweis*", "fuhrerschein*", "sozialversicherung*",
    "rentenversicherung*", "versicherungsnummer", "sv nummer", "steuer id", "steuerid*", "steuer nr", "steuernummer",
    "steueridentifikationsnummer", "identifikationsnummer", "idnr", "kreditkarte*", "kartennummer",
    // es
    "pasaporte", "carnet de conducir", "permiso de conducir", "licencia de conducir", "seguridad social", "tarjeta",
    "dni", "nif", "cif", "nss",
    // all
    "iban", "mrz",
];

/// Short words that are an ID's name only in capitals (« nie » is German for « never », « niño » folds to « nino »).
const CAPITALS: &[&str] = &["NIE", "NINO"];

/// Words that name a bank card or account (folded): a long number near one is the person's, whatever its check digit.
const BANK_WORDS: &[&str] = &[
    "carte", "card", "karte*", "kreditkarte*", "tarjeta", "cb", "visa", "mastercard", "amex", "maestro", "bancaire",
    "rib", "iban", "virement", "konto*", "cuenta",
];

/// Words of a parcel or an order (folded): their long numbers are no ID numbers (a card's or an account's still is).
const PARCEL_WORDS: &[&str] = &[
    "colis", "suivi", "commande", "livraison", "parcel", "tracking", "order", "shipment", "delivery", "sendung*",
    "paket", "bestellung*", "lieferung", "paquete", "envio", "pedido", "seguimiento", "dhl", "ups", "fedex", "gls",
    "colissimo", "chronopost", "dpd", "amazon",
];

/// Each IBAN country's length (SWIFT's IBAN registry).
const IBAN_LENGTHS: &[(&str, usize)] = &[
    ("AD", 24), ("AE", 23), ("AL", 28), ("AT", 20), ("AZ", 28), ("BA", 20), ("BE", 16), ("BG", 22), ("BH", 22),
    ("BI", 27), ("BR", 29), ("BY", 28), ("CH", 21), ("CR", 22), ("CY", 28), ("CZ", 24), ("DE", 22), ("DJ", 27),
    ("DK", 18), ("DO", 28), ("EE", 20), ("EG", 29), ("ES", 24), ("FI", 18), ("FK", 18), ("FO", 18), ("FR", 27),
    ("GB", 22), ("GE", 22), ("GI", 23), ("GL", 18), ("GR", 27), ("GT", 28), ("HN", 28), ("HR", 21), ("HU", 28),
    ("IE", 22), ("IL", 23), ("IQ", 23), ("IS", 26), ("IT", 27), ("JO", 30), ("KW", 30), ("KZ", 20), ("LB", 28),
    ("LC", 32), ("LI", 21), ("LT", 20), ("LU", 20), ("LV", 21), ("LY", 25), ("MC", 27), ("MD", 24), ("ME", 22),
    ("MK", 19), ("MN", 20), ("MR", 27), ("MT", 31), ("MU", 30), ("NI", 28), ("NL", 18), ("NO", 15), ("OM", 23),
    ("PK", 24), ("PL", 28), ("PS", 29), ("PT", 25), ("QA", 29), ("RO", 24), ("RS", 22), ("RU", 33), ("SA", 24),
    ("SC", 31), ("SD", 18), ("SE", 24), ("SI", 19), ("SK", 24), ("SM", 27), ("SO", 23), ("ST", 25), ("SV", 28),
    ("TL", 23), ("TN", 24), ("TR", 26), ("UA", 29), ("VA", 22), ("VG", 24), ("XK", 20), ("YE", 30),
];

/// E-mail addresses (a site's login is often one) and web addresses: set aside before any check.
static ADDRESSES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\w.%+\-]+@[\w\-]+(?:\.[\w\-]+)*\.[a-z]{2,}|\bhttps?://\S+|\bwww\.\S+").expect("valid regex")
});

/// A French social security number (NIR) written in its groups (« 1 85 05 78 006 084 36 », « 2.05.12.75.116.001.42 »),
/// looked for before the dates are set aside (« 2.05.12 » could be one).
static NIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[1-478][ .\-]\d{2}[ .\-](?:0[1-9]|1[0-2]|[2-9]\d)[ .\-](?:\d{2}|2[ABab])[ .\-]\d{3}[ .\-]\d{3}(?:[ .\-]\d{2})?\b")
        .expect("valid regex")
});

/// A bank card's shape: four groups of four (the last 1 to 4), or Amex's 4-6-5, apart by a dash, a dot or spaces.
static CARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{4}(?:\s{1,3}|[.\-])\d{4}(?:\s{1,3}|[.\-])\d{4}(?:\s{1,3}|[.\-])\d{1,4}\b|\b3[47]\d{2}(?:\s{1,3}|[.\-])\d{6}(?:\s{1,3}|[.\-])\d{5}\b")
        .expect("valid regex")
});

/// Dates in numbers (« 14/03/1991 », « 14.03.2026 », « 2031-03-12 »): set aside when they can be one (`a_date`).
static NUMBER_DATES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,4})[./\-](\d{1,2})[./\-](\d{1,4})\b").expect("valid regex"));

/// A month and its year (« 03.2031 », « 03/2031 », « 2031-03 »): a validity, never an ID.
static MONTH_YEARS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:0?[1-9]|1[0-2])[./\-](?:19|20)\d{2}\b|\b(?:19|20)\d{2}[./\-](?:0[1-9]|1[0-2])\b").expect("valid regex")
});

/// Dates with the month's name, in the four languages (« 14 novembre 2026 », « 14 March 2026 », « March 14, 2026 »,
/// « 14. März 2026 », « 14 de marzo de 2026 »).
static NAMED_DATES: LazyLock<Regex> = LazyLock::new(|| {
    const MONTHS: &str = "jan|janv|janvier|january|januar|janner|jänner|enero|feb|fev|fevr|fevrier|févr|février|\
february|februar|febrero|mar|mars|march|marz|märz|marzo|apr|avr|avril|april|abr|abril|may|mai|mayo|jun|juin|june|juni|\
junio|jul|juil|juillet|july|juli|julio|aug|aou|aout|août|august|ago|agosto|sep|sept|septembre|september|septiembre|oct|\
okt|octobre|october|oktober|octubre|nov|novembre|november|noviembre|dec|dez|déc|decembre|décembre|december|dezember|\
diciembre|dic";
    Regex::new(&format!(
        r"(?i)\b\d{{1,2}}(?:er|st|nd|rd|th)?\.?\s+(?:de\s+)?(?:{MONTHS})\.?\s+(?:de\s+)?\d{{2,4}}\b|\
\b(?:{MONTHS})\.?\s+\d{{1,2}}(?:st|nd|rd|th)?,?\s+\d{{4}}\b"
    ))
    .expect("valid regex")
});

/// Times of day (« 14h30 », « 10:30 », « 9.00 », « 9 h »): set aside too.
static TIMES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:[01]?\d|2[0-3])\s?[h:.]\s?[0-5]\d\b|\b\d{1,2}\s?h\b").expect("valid regex")
});

/// A phone number after its word (« tél. 01 23 45 67 89 », « Telefon 030 1234567 », « móvil: +34 612 345 678 »).
static PHONES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:t[ée]l(?:[ée]phone)?|telefon|phone|mobile?|handy|portable|m[óo]vil|tel[ée]fono|fax|fixe)\b\.?\s*:?\s*\+?[\d(][\d .\-/()]{4,}\d")
        .expect("valid regex")
});

/// A book's number: after « ISBN », or an ISBN-13 (978/979) whose check digit holds.
static ISBN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bISBN(?:-1[03])?\s*:?\s*[\dX][\d\- X]{8,16}[\dX]\b|\b97[89]\d{10}\b").expect("valid regex")
});

/// A Spanish DNI (8 digits and its letter) or NIE (X, Y or Z, 7 digits and its letter): its letter checks it.
static DNI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([XYZ]?)(\d{7,8})[\- ]?([A-Z])\b").expect("valid regex"));

/// An ID number in a fact: never kept.
pub fn id_number(text: &str) -> bool {
    let text = ADDRESSES.replace_all(text, " ");
    if NIR.is_match(&text) || cards(&text) {
        return true;
    }
    let text = set_aside(&text);
    long_digits(&text) || iban(&text) || dni(&text) || (named(&text) && coded(&text))
}

/// Whether three numbers can be a date: day, month and year (or year, month, day; or month, day, year), a year of
/// four digits between 1900 and 2100 (« 012-05-1120 » is a social security number, not a date).
fn a_date(a: &str, b: &str, c: &str) -> bool {
    let n = |s: &str| s.parse::<u32>().unwrap_or(u32::MAX);
    let year = |s: &str| s.len() == 2 || (s.len() == 4 && (1900..=2100).contains(&n(s)));
    let day_month = |d: u32, m: u32| (1..=31).contains(&d) && (1..=12).contains(&m);
    if a.len() == 4 {
        return year(a) && c.len() <= 2 && day_month(n(c), n(b));
    }
    a.len() <= 2 && year(c) && (day_month(n(a), n(b)) || day_month(n(b), n(a)))
}

/// The text with its e-mail and web addresses, its phone numbers (after their word), books, dates and times set aside.
fn set_aside(text: &str) -> String {
    let text = ADDRESSES.replace_all(text, " ");
    let text = PHONES.replace_all(&text, " ");
    let text = ISBN.replace_all(&text, |c: &regex::Captures| {
        let found = &c[0];
        let digits: String = found.chars().filter(char::is_ascii_digit).collect();
        if found.to_ascii_uppercase().starts_with("ISBN") || isbn13(&digits) { " ".to_string() } else { found.to_string() }
    });
    let text = NAMED_DATES.replace_all(&text, " ");
    let text = NUMBER_DATES.replace_all(&text, |c: &regex::Captures| {
        if a_date(&c[1], &c[2], &c[3]) { " ".to_string() } else { c[0].to_string() }
    });
    let text = MONTH_YEARS.replace_all(&text, " ");
    TIMES.replace_all(&text, " ").into_owned()
}

/// An ISBN-13 whose check digit holds.
fn isbn13(digits: &str) -> bool {
    let d: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    d.len() == 13 && d.iter().enumerate().map(|(i, x)| if i % 2 == 0 { *x } else { 3 * x }).sum::<u32>().is_multiple_of(10)
}

/// A number whose last digit checks it as a card's does (Luhn).
fn luhn(digits: &str) -> bool {
    let d: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, x)| if i % 2 == 1 { if *x * 2 > 9 { *x * 2 - 9 } else { *x * 2 } } else { *x })
        .sum();
    !d.is_empty() && sum.is_multiple_of(10)
}

/// Folded words of `list` in `text` (`*`: a word starting so).
fn holds(text: &str, list: &[&str]) -> bool {
    let padded = format!(" {} ", fold(text));
    list.iter().any(|k| match k.strip_suffix('*') {
        Some(stem) => padded.split(' ').any(|w| !w.is_empty() && w.starts_with(stem)),
        None => padded.contains(&format!(" {k} ")),
    })
}

/// A bank card written in its groups (`CARD`): a card's check digit, or a card or an account named in the text.
fn cards(text: &str) -> bool {
    let named = holds(text, BANK_WORDS);
    CARD.find_iter(text).any(|m| named || luhn(&m.as_str().chars().filter(char::is_ascii_digit).collect::<String>()))
}

/// A long run of digits (13 or more, groups apart by up to three spaces): a card, a social security or tax number, an
/// account. A phone number (after « + » or « 00 »: 15 digits at most) is left out, and so is a parcel's or an order's
/// number (`PARCEL_WORDS`) unless it checks as a card's, or a card, an account or an ID is named.
fn long_digits(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let parcel = holds(text, PARCEL_WORDS) && !holds(text, BANK_WORDS) && !named(text);
    let space = |c: char| matches!(c, ' ' | '\u{a0}' | '\u{202f}');
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut digits = String::new();
        while i < chars.len() {
            if chars[i].is_ascii_digit() {
                digits.push(chars[i]);
                i += 1;
                continue;
            }
            // a gap of one to three spaces between two digits joins them
            let gap = chars[i..].iter().take_while(|c| space(**c)).count();
            if (1..=3).contains(&gap) && chars.get(i + gap).is_some_and(char::is_ascii_digit) {
                i += gap;
                continue;
            }
            break;
        }
        let after_plus = start > 0 && chars[..start].iter().rev().find(|c| !c.is_whitespace()) == Some(&'+');
        let phone = (after_plus && digits.len() <= 15) || (digits.starts_with("00") && digits.len() <= 17);
        let card = (13..=19).contains(&digits.len()) && luhn(&digits);
        if digits.len() >= 13 && !phone && !(parcel && !card) {
            return true;
        }
    }
    false
}

/// An IBAN: a country's two letters and two check digits, then its account, in one block or in groups of four (« FR76
/// 3000 6000 0112 3456 7890 189 »), the country's own length in all, and at least half the account in digits.
fn iban(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == '-')
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_ascii_uppercase())
        .collect();
    let fits = |code: &str| {
        let Some((_, length)) = IBAN_LENGTHS.iter().find(|(country, _)| code.get(..2) == Some(*country)) else {
            return false;
        };
        let account = &code[4..];
        code.len() == *length && account.chars().filter(char::is_ascii_digit).count() * 2 >= account.len()
    };
    let head = |w: &str| {
        let b = w.as_bytes();
        b.len() >= 4 && b[0].is_ascii_uppercase() && b[1].is_ascii_uppercase() && b[2].is_ascii_digit() && b[3].is_ascii_digit()
    };
    let plain = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric());
    for (i, word) in words.iter().enumerate() {
        // in one block
        if head(word) && plain(word) && fits(word) {
            return true;
        }
        // in groups: « FR76 3000 … » or « FR 76 3000 … »
        let (mut code, mut next) = if word.len() == 4 && head(word) {
            (word.clone(), i + 1)
        } else if word.len() == 2
            && word.chars().all(|c| c.is_ascii_uppercase())
            && words.get(i + 1).is_some_and(|w| w.len() == 2 && w.chars().all(|c| c.is_ascii_digit()))
        {
            (format!("{word}{}", words[i + 1]), i + 2)
        } else {
            continue;
        };
        while let Some(group) = words.get(next).filter(|w| plain(w) && w.len() <= 4) {
            code.push_str(group);
            if fits(&code) {
                return true;
            }
            if group.len() < 4 {
                break; // only the last group is shorter
            }
            next += 1;
        }
    }
    false
}

/// A Spanish DNI or NIE whose letter checks (`DNI`).
fn dni(text: &str) -> bool {
    const LETTERS: &[u8] = b"TRWAGMYFPDXBNJZSQVHLCKE";
    DNI.captures_iter(&text.to_uppercase()).any(|c| {
        let (prefix, digits, letter) = (&c[1], &c[2], c[3].as_bytes()[0]);
        let lead = match prefix {
            "" if digits.len() == 8 => "",
            "X" if digits.len() == 7 => "0",
            "Y" if digits.len() == 7 => "1",
            "Z" if digits.len() == 7 => "2",
            _ => return false,
        };
        format!("{lead}{digits}").parse::<u64>().is_ok_and(|n| LETTERS[(n % 23) as usize] == letter)
    })
}

/// An ID's name in the text (`KEYS`, `CAPITALS`).
fn named(text: &str) -> bool {
    holds(text, KEYS) || text.split(|c: char| !c.is_alphanumeric()).any(|w| CAPITALS.contains(&w))
}

/// A code next to an ID's name: 6 characters or more, with 4 digits or more, or 8 or more mixing 2 digits and 2
/// letters or more (« 12AB34567 », « 12 AB 34567 », « X4RTBPFW4 », « 123-45-6789 », « 21/815/08150 », « QQ 12 34 56
/// C »): the stretches of short groups that hold a digit or are a few capitals, apart by spaces, dashes, dots or
/// slashes.
fn coded(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let joins = |c: char| matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '-' | '.' | '/');
    let mut runs: Vec<Vec<String>> = vec![Vec::new()];
    let mut group = String::new();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_alphanumeric() {
            group.push(*c);
            continue;
        }
        let inside = joins(*c) && !group.is_empty() && chars.get(i + 1).is_some_and(char::is_ascii_alphanumeric);
        if let Some(run) = runs.last_mut() {
            run.push(std::mem::take(&mut group));
        }
        if !inside {
            runs.push(Vec::new());
        }
    }
    if let Some(run) = runs.last_mut() {
        run.push(group);
    }
    let end = String::new();
    runs.iter().any(|run| {
        let mut code = String::new();
        let mut found = false;
        for group in run.iter().chain(std::iter::once(&end)) {
            let digit = group.chars().any(|c| c.is_ascii_digit());
            let capitals = !group.is_empty() && group.len() <= 3 && group.chars().all(|c| c.is_ascii_uppercase());
            if digit || capitals {
                code.push_str(group);
            } else {
                let digits = code.chars().filter(char::is_ascii_digit).count();
                let letters = code.chars().filter(char::is_ascii_alphabetic).count();
                found |= code.len() >= 6 && (digits >= 4 || (digits >= 2 && letters >= 2 && code.len() >= 8));
                code.clear();
            }
        }
        found
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 1 (14/11): real ID numbers the first version let into the Carnet, under their everyday names and forms.
    #[test]
    fn the_everyday_names_and_forms_of_id_numbers_are_refused() {
        let kept: Vec<&str> = [
            // German tax number as on every Steuerbescheid (slashes)
            "Steuernummer: 21/815/08150",
            "Steuer-Nr. 181/815/08155",
            "SV-Nummer: 65 170839 J 003",
            "Pass-Nr.: C01X00T47",
            "Identifikationsnummer: 12 345 678 901",
            "Driving license: 880692310285",
            "Driver License: D1234567",
            "ID number: 880692310285",
            "Pièce d'identité n° X4RTBPFW4",
            // as copied from a PDF: two spaces between the groups
            "Carte bancaire 4970  1012  3456  7890",
            // a child's social security number with dots (« 2.05.12 » could be a date): the shape of a NIR
            "Zoé : 2.05.12.75.116.001.42",
            "Sécu de Léa : 1.85.05.78.006.084.36",
        ]
        .into_iter()
        .filter(|t| !id_number(t))
        .collect();
        assert!(kept.is_empty(), "kept by the Carnet: {kept:#?}");
    }

    /// Review 2 (14/11): « permis » alone (the French everyday name of the driving licence), a RIB with dashes or dots,
    /// a card right after a phone word.
    #[test]
    fn a_permis_a_rib_and_a_card_after_a_phone_word_are_refused() {
        let kept: Vec<&str> = [
            "N° de permis : 12AA34567",
            "Numéro de permis de Léa : 12AA34567",
            "Permis n° 15AB12345",
            "RIB : 30006-00001-12345678901-89",
            "RIB 30006.00001.12345678901.89",
            "Handy 4111111111111111",
            "Tél. 5500000000000004",
        ]
        .into_iter()
        .filter(|t| !id_number(t))
        .collect();
        assert!(kept.is_empty(), "kept by the Carnet: {kept:#?}");
    }

    /// Review 2 (14/11), refused before this branch too: a flight next to « passeport », lists of postcodes or years, a
    /// company's SIRET, a customer number; a building permit is no driving licence.
    #[test]
    fn flights_lists_companies_customers_and_building_permits_are_kept() {
        let refused: Vec<&str> = [
            "Vol AF1234 le 12 mars, passeport à prendre",
            "Codes postaux du secteur : 75011 75012 75020",
            "Vacances en 2019 2021 2023 2025",
            "SIRET de l'artisan 123 456 789 00012",
            "Numéro client Free 12345678901234",
            "Permis de construire PC 069 123 22 A0045 accordé",
        ]
        .into_iter()
        .filter(|t| id_number(t))
        .collect();
        assert!(refused.is_empty(), "refused by the Carnet: {refused:#?}");
    }

    /// Review 1 (14/11): ordinary notes the first version refused (a book, an order, a validity, opening hours, a
    /// phone number next to an ID word, a parcel).
    #[test]
    fn books_orders_validities_hours_phones_and_parcels_are_kept() {
        let refused: Vec<&str> = [
            "Livre à acheter pour Léa : ISBN 978-2-07-036822-8",
            "Livre : 9782070368228",
            "Commande Amazon 402-9384756-1029384, livraison prévue jeudi",
            "Reisepass von Léa gültig bis 03.2031",
            "Passport expires 2031-03, renew in January",
            "Bürgeramt (Reisepass): geöffnet 9.00-12.00 Uhr",
            "RDV passeport à la mairie le 3 décembre, tél. 01 23 45 67 89",
            "Reisepass: Termin beim Bürgeramt, Telefon 030 1234567",
            "Colis DHL 00340434161094042557, livré au point relais",
            "Suivi du colis : 6A12345678901, livré mardi",
        ]
        .into_iter()
        .filter(|t| id_number(t))
        .collect();
        assert!(refused.is_empty(), "refused by the Carnet: {refused:#?}");
    }

    #[test]
    fn each_check_alone() {
        assert!(long_digits("4970 1012 3456 7890") && cards("Carte 4970-1012-3456-7890") && cards("4111.1111.1111.1111"));
        assert!(!long_digits("402-9384756-1029384") && !cards("402-9384756-1029384") && !cards("2026 2027 2028 2029"));
        assert!(NIR.is_match("2.05.12.75.116.001.42") && !NIR.is_match("402-9384756-1029384"));
        assert!(!long_digits("Colis 00340434161094042557") && long_digits("Commande payée par carte 4970101234567890"));
        assert!(!long_digits("+49 151 23456789") && !long_digits("0049 151 23456789") && !long_digits("06 12 34 56 78"));
        assert!(iban("FR76 3000 6000 0112 3456 7890 189") && iban("fr7630006000011234567890189") && iban("BE 68 5390 0754 7034"));
        assert!(!iban("LE14NOVEMBRE2026SURLAPHARMACIE") && !iban("Lu 12 fois avec Paul dans le train"));
        assert!(!iban("FR76 3000 6000 0112 3456 7890"), "one group short of a French IBAN");
        assert!(dni("12345678Z") && dni("X1234567L") && !dni("12345678A"));
        assert!(named("Mon NIE") && !named("Er ruft nie zurück") && named("Sozialversicherungsnummer"));
        assert!(coded("SSN 123-45-6789") && coded("X4RTBPFW4") && !coded(&set_aside("RDV 14h30 guichet B12")));
        let dated = set_aside("née le 14/03/1991, le 14 novembre 2026, 2031-03-12, a.b@c.fr, https://x.fr/123");
        assert!(!dated.chars().any(|c| c.is_ascii_digit() || c == '@'), "{dated}");
        for kept in ["012-05-1120", "1.85.05.78", "123-45-6789"] {
            assert_eq!(set_aside(kept), kept);
        }
    }
}
