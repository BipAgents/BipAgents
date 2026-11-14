//! What the Carnet never keeps: ID numbers (passports, ID cards, social security, tax numbers, driving licences, bank
//! cards, IBAN), in the usual forms of the four languages (`lexicon::id_number`). An e-mail address or a site's login
//! is not one, nor a date or a phone number.
//!
//! bridge-tools-no-breaker (14/11): on the sign-up bench, DeepSeek's note « Compte client créé le 14 novembre 2026 sur
//! la Pharmacie des Essais… » was refused three times in a row: the IBAN check squeezed the spaces out of the whole
//! text and read « LE14NOVEMBRE2026SURLA… » as one (two letters, two digits, eleven more). The agent took its e-mail
//! login for the culprit, took it out, and was refused again; Hermes then paused all the bridge's tools for a minute.
//! Now an IBAN is a country's code with its own length, written in one block or in groups of four, mostly digits;
//! e-mail and web addresses and dates are set aside before any check; short words that are also common words (« nie »:
//! German for « never ») count only in capitals.

use std::sync::LazyLock;

use regex::Regex;

use super::fold;

/// Words that say an ID number follows, folded (`fold`); `*`: any word starting so (the German compounds).
const KEYS: &[&str] = &[
    // fr
    "passeport", "carte d identite", "carte identite", "carte nationale d identite", "titre de sejour",
    "permis de conduire", "securite sociale", "secu", "carte vitale", "numero fiscal", "carte bancaire",
    "numero de carte", "cni", "nir",
    // en
    "passport", "id card", "identity card", "driving licence", "driver s license", "drivers license",
    "social security", "ssn", "national insurance", "ni number", "card number", "credit card", "tax id",
    // de
    "reisepass*", "passnummer", "personalausweis*", "ausweis*", "fuhrerschein*", "sozialversicherung*",
    "rentenversicherung*", "versicherungsnummer", "steuer id", "steuerid*", "steuernummer",
    "steueridentifikationsnummer", "kreditkarte*", "kartennummer",
    // es
    "pasaporte", "carnet de conducir", "permiso de conducir", "seguridad social", "tarjeta", "dni", "nif", "cif",
    "nss",
    // all
    "iban", "mrz",
];

/// Short words that are an ID's name only in capitals (« nie » is German for « never », « niño » folds to « nino »).
const CAPITALS: &[&str] = &["NIE", "NINO"];

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

/// Dates in numbers (« 14/03/1991 », « 14.03.2026 », « 2031-03-12 »): set aside when they can be one (`a_date`).
static NUMBER_DATES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,4})[./\-](\d{1,2})[./\-](\d{1,4})\b").expect("valid regex"));

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

/// Times of day (« 14h30 », « 10:30 », « 9 h »): set aside too.
static TIMES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b\d{1,2}\s?[h:]\s?\d{2}\b|\b\d{1,2}\s?h\b").expect("valid regex"));

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

/// The text with its e-mail and web addresses, its dates and times set aside.
fn set_aside(text: &str) -> String {
    let text = ADDRESSES.replace_all(text, " ");
    let text = NAMED_DATES.replace_all(&text, " ");
    let text = TIMES.replace_all(&text, " ");
    NUMBER_DATES
        .replace_all(&text, |c: &regex::Captures| {
            if a_date(&c[1], &c[2], &c[3]) { " ".to_string() } else { c[0].to_string() }
        })
        .into_owned()
}

/// A Spanish DNI (8 digits and its letter) or NIE (X, Y or Z, 7 digits and its letter): its letter checks it.
static DNI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([XYZ]?)(\d{7,8})[\- ]?([A-Z])\b").expect("valid regex"));

/// An ID number in a fact: never kept.
pub fn id_number(text: &str) -> bool {
    let text = set_aside(text);
    long_digits(&text) || iban(&text) || dni(&text) || (named(&text) && coded(&text))
}

/// A digit's joiner inside a long number: one space (a no-break one too), a dash or a dot between two digits.
fn joins(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '-' | '.')
}

/// A long run of digits (13 or more, groups joined by `joins`): a card, a social security or tax number, an account.
/// A phone number (after « + » or « 00 »: 15 digits at most) is left out.
fn long_digits(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
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
            } else if !(joins(chars[i]) && i > 0 && chars[i - 1].is_ascii_digit() && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
                break;
            }
            i += 1;
        }
        let after_plus = start > 0 && chars[..start].iter().rev().find(|c| !c.is_whitespace()) == Some(&'+');
        let phone = (after_plus && digits.len() <= 15) || (digits.starts_with("00") && digits.len() <= 17);
        if digits.len() >= 13 && !phone {
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
    let padded = format!(" {} ", fold(text));
    let key = KEYS.iter().any(|k| match k.strip_suffix('*') {
        Some(stem) => padded.split(' ').any(|w| !w.is_empty() && w.starts_with(stem)),
        None => padded.contains(&format!(" {k} ")),
    });
    key || text.split(|c: char| !c.is_alphanumeric()).any(|w| CAPITALS.contains(&w))
}

/// A code next to an ID's name: 6 characters or more, 2 digits or more (« 12AB34567 », « 12 AB 34567 », « X4RTBPFW4 »,
/// « 123-45-6789 », « QQ 12 34 56 C »): the stretches of short groups that hold a digit or are a few capitals.
fn coded(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut runs: Vec<Vec<String>> = vec![Vec::new()];
    let mut group = String::new();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_alphanumeric() {
            group.push(*c);
            continue;
        }
        let inside = joins(*c)
            && !group.is_empty()
            && chars.get(i + 1).is_some_and(char::is_ascii_alphanumeric);
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
                found |= code.len() >= 6 && digits >= 2;
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
        assert!(long_digits("4970 1012 3456 7890") && long_digits("4970-1012-3456-7890"));
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
