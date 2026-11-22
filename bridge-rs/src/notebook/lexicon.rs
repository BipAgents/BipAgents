//! The Carnet's small lexicon (Carnet v2, socle §0 of internal/notes/carnet-v2/spec.md), in the four languages, all
//! folded (`fold`: lower case, no accents): what a « Libellé : valeur » fact's label means (`LABELS`, used at the
//! write to reshape « Chausse du 36 » into « Pointure : 36 » and to replace a label said again, and at the search to
//! answer « Léa chausse du combien ? »), the words a search drops (`STOP`), and the words that make a fact a health
//! fact (`is_health`: never in the shared part) or a money one (`is_money`). Plus the checks of what is never kept (ID
//! numbers) and the person's contact forms (e-mail, phone in E.164).

use super::fold;

/// A label of a short fact, its name in fr, en, de, es (the Carnet's language picks one), and the phrases that mean
/// it (folded). `joins`: phrases that start a fact and lead straight to its value (« chausse du 36 »); a label without
/// any is never written in its form by the bridge (`reshape`), only recognised.
pub struct Label {
    pub id: &'static str,
    pub names: [&'static str; 4],
    pub words: &'static [&'static str],
    pub joins: &'static [&'static str],
    /// A health label (its facts go to the private health part).
    pub health: bool,
}

const fn label(
    id: &'static str,
    names: [&'static str; 4],
    words: &'static [&'static str],
    joins: &'static [&'static str],
) -> Label {
    Label { id, names, words, joins, health: false }
}

const fn health_label(
    id: &'static str,
    names: [&'static str; 4],
    words: &'static [&'static str],
    joins: &'static [&'static str],
) -> Label {
    Label { id, names, words, joins, health: true }
}

/// About thirty labels, those people restate (one value each: a new one replaces the old).
pub const LABELS: &[Label] = &[
    label(
        "shoe_size",
        ["Pointure", "Shoe size", "Schuhgröße", "Talla de zapatos"],
        &["pointure", "chausse", "taille de chaussures", "shoe size", "schuhgrosse", "talla de zapatos", "calza"],
        &["chausse du", "chausse", "taille de chaussures", "wears size", "shoe size", "schuhgrosse", "calza el", "calza un", "calza"],
    ),
    label(
        "birthday",
        ["Anniversaire", "Birthday", "Geburtstag", "Cumpleaños"],
        &["anniversaire", "anniv", "date de naissance", "ne le", "nee le", "birthday", "date of birth", "born on",
          "geburtstag", "geboren am", "cumpleanos", "fecha de nacimiento", "nacio el", "nacida el", "nacido el"],
        &["ne le", "nee le", "date de naissance", "born on", "date of birth", "geboren am", "nacio el", "nacida el", "nacido el"],
    ),
    label(
        "height",
        ["Taille", "Height", "Größe", "Altura"],
        &["taille", "mesure", "height", "tall", "grosse", "altura", "mide"],
        &["mesure", "is tall", "ist gross"],
    ),
    label(
        "clothing_size",
        ["Taille de vêtements", "Clothing size", "Kleidergröße", "Talla de ropa"],
        &["taille de vetements", "taille vetements", "clothing size", "clothes size", "kleidergrosse", "konfektionsgrosse", "talla de ropa"],
        &["taille de vetements", "clothing size", "kleidergrosse", "talla de ropa"],
    ),
    label(
        "ring_size",
        ["Tour de doigt", "Ring size", "Ringgröße", "Talla de anillo"],
        &["tour de doigt", "taille de bague", "ring size", "ringgrosse", "talla de anillo"],
        &["tour de doigt", "taille de bague", "ring size", "ringgrosse", "talla de anillo"],
    ),
    label(
        "name_day",
        ["Fête", "Name day", "Namenstag", "Santo"],
        &["fete", "name day", "namenstag", "santo", "onomastica"],
        &["fete le", "fetee le", "name day on", "namenstag am", "su santo es el"],
    ),
    label(
        "wedding",
        ["Anniversaire de mariage", "Wedding anniversary", "Hochzeitstag", "Aniversario de boda"],
        &["anniversaire de mariage", "mariage", "marie le", "maries le", "wedding anniversary", "wedding", "married on",
          "hochzeitstag", "hochzeit", "aniversario de boda", "boda", "casados el"],
        &["maries le", "marie le", "mariee le", "married on", "verheiratet seit", "casados el"],
    ),
    label(
        "phone",
        ["Téléphone", "Phone", "Telefon", "Teléfono"],
        &["telephone", "tel", "portable", "phone", "mobile", "cell", "telefon", "handy", "handynummer", "telefono", "movil", "celular"],
        &[],
    ),
    label(
        "email",
        ["E-mail", "E-mail", "E-Mail", "Correo"],
        &["e mail", "email", "mail", "courriel", "adresse mail", "adresse e mail", "correo", "correo electronico"],
        &[],
    ),
    label(
        "address",
        ["Adresse", "Address", "Adresse", "Dirección"],
        &["adresse", "address", "anschrift", "direccion", "domicilio"],
        &[],
    ),
    label(
        "city",
        ["Ville", "City", "Stadt", "Ciudad"],
        &["ville", "city", "stadt", "wohnort", "ciudad"],
        &[],
    ),
    label(
        "job",
        ["Métier", "Job", "Beruf", "Profesión"],
        &["metier", "profession", "job", "occupation", "beruf", "profesion", "oficio", "trabajo"],
        &[],
    ),
    label(
        "employer",
        ["Employeur", "Employer", "Arbeitgeber", "Empresa"],
        &["employeur", "employer", "arbeitgeber", "empresa"],
        &[],
    ),
    label(
        "partner",
        ["Conjoint", "Partner", "Partner", "Pareja"],
        &["conjoint", "conjointe", "compagnon", "compagne", "epoux", "epouse", "partner", "husband", "wife", "spouse",
          "ehemann", "ehefrau", "partnerin", "pareja", "marido", "esposa", "esposo"],
        &[],
    ),
    label(
        "children",
        ["Enfants", "Children", "Kinder", "Hijos"],
        &["enfants", "children", "kids", "kinder", "hijos"],
        &[],
    ),
    label(
        "pet",
        ["Animal", "Pet", "Haustier", "Mascota"],
        &["animal", "animal de compagnie", "pet", "haustier", "mascota"],
        &[],
    ),
    label(
        "favourite_colour",
        ["Couleur préférée", "Favourite colour", "Lieblingsfarbe", "Color favorito"],
        &["couleur preferee", "favourite colour", "favorite color", "lieblingsfarbe", "color favorito", "color preferido"],
        &["couleur preferee", "favourite colour", "favorite color", "lieblingsfarbe", "color favorito"],
    ),
    label(
        "favourite_food",
        ["Plat préféré", "Favourite food", "Lieblingsessen", "Comida favorita"],
        &["plat prefere", "favourite food", "favorite food", "favorite dish", "lieblingsessen", "lieblingsgericht", "comida favorita", "plato favorito"],
        &["plat prefere", "favourite food", "favorite food", "lieblingsessen", "comida favorita"],
    ),
    label(
        "favourite_drink",
        ["Boisson préférée", "Favourite drink", "Lieblingsgetränk", "Bebida favorita"],
        &["boisson preferee", "favourite drink", "favorite drink", "lieblingsgetrank", "bebida favorita"],
        &["boisson preferee", "favourite drink", "favorite drink", "lieblingsgetrank", "bebida favorita"],
    ),
    label(
        "diet",
        ["Régime alimentaire", "Diet", "Ernährung", "Dieta"],
        &["regime alimentaire", "regime", "diet", "ernahrung", "dieta"],
        &[],
    ),
    label(
        "school",
        ["École", "School", "Schule", "Colegio"],
        &["ecole", "college", "lycee", "school", "schule", "colegio", "escuela"],
        &[],
    ),
    label(
        "class",
        ["Classe", "School year", "Klasse", "Curso"],
        &["classe", "school year", "grade", "klasse", "curso"],
        &[],
    ),
    label(
        "car",
        ["Voiture", "Car", "Auto", "Coche"],
        &["voiture", "car", "auto", "coche"],
        &[],
    ),
    label(
        "language",
        ["Langue", "Language", "Sprache", "Idioma"],
        &["langue", "language", "sprache", "idioma"],
        &[],
    ),
    label(
        "nickname",
        ["Surnom", "Nickname", "Spitzname", "Apodo"],
        &["surnom", "nickname", "spitzname", "apodo"],
        &[],
    ),
    health_label(
        "allergies",
        ["Allergies", "Allergies", "Allergien", "Alergias"],
        &["allergie", "allergies", "allergique", "allergy", "allergies", "allergic", "allergien", "alergia", "alergias"],
        &[],
    ),
    health_label(
        "blood_type",
        ["Groupe sanguin", "Blood type", "Blutgruppe", "Grupo sanguíneo"],
        &["groupe sanguin", "blood type", "blood group", "blutgruppe", "grupo sanguineo"],
        &["groupe sanguin", "blood type", "blutgruppe", "grupo sanguineo"],
    ),
    health_label(
        "doctor",
        ["Médecin", "Doctor", "Arzt", "Médico"],
        &["medecin", "medecin traitant", "docteur", "doctor", "gp", "arzt", "hausarzt", "medico"],
        &[],
    ),
];

/// Words a search drops (any of the four languages: a question may mix them).
pub const STOP: &[&str] = &[
    // fr
    "de", "du", "la", "le", "les", "des", "d", "l", "un", "une", "au", "aux", "et", "a", "en", "pour", "sur", "ma", "mon",
    "mes", "ta", "ton", "tes", "sa", "son", "ses", "quel", "quelle", "quels", "quelles", "est", "c", "qu", "que", "qui",
    "quoi", "combien", "elle", "il", "quand",
    // en
    "the", "of", "an", "and", "to", "is", "what", "which", "who", "how", "many", "much", "my", "your", "his", "her",
    "does", "s", "when", "has",
    // de
    "der", "die", "das", "den", "dem", "von", "ein", "eine", "und", "ist", "was", "wie", "welche", "hat", "wann",
    // es
    "el", "los", "las", "del", "y", "es", "cual", "cuanto", "cuando", "tiene",
];

/// Health words (folded) that say health on their own: a word that starts with one of them, or a phrase; the stems
/// end with `*`.
const HEALTH: &[&str] = &[
    // fr
    "allerg*", "medicament*", "maladi*", "malade", "malades", "ordonnance*", "medecin*", "hopital", "hopitaux",
    "grossesse*", "enceinte de", "diabet*", "asthm*", "psychiatr*", "psychologue*", "therapeute*", "vaccin*",
    "tension arterielle", "cholesterol", "migraine*", "epileps*", "insomni*", "kine", "kines", "kinesitherap*",
    "dentist*", "gyneco*", "pediatr*", "ophtalmo*", "orthodont*", "osteopath*", "thyroid*", "groupe sanguin",
    "intoleran*", "coeliaq*", "perte de poids", "prise de poids", "sous traitement", "traitement pour",
    "traitement contre", "en traitement", "opere du", "operee du", "opere de la", "operee de la", "opere d", "operee d",
    "cancer du", "cancer de", "en depression", "blesse au", "blessee au", "blessure au", "douleur au", "douleurs au",
    "mal au dos", "mal de dos", "en therapie", "seance de therapie", "seances de therapie",
    // en
    "medication*", "prescription*", "doctor*", "hospital*", "pregnan*", "asthma", "blood pressure", "migraines",
    "epilep*", "insomnia", "blood type", "celiac", "weight loss", "illness*", "disease*", "physiotherap*",
    "under treatment", "treatment for",
    // de
    "medikament*", "krankheit*", "krank", "arzt", "arztin", "krankenhaus*", "schwanger*", "impf*", "blutdruck",
    "cholesterin", "migran*", "schlaflos*", "zahnarzt*", "blutgruppe", "unvertraglich*",
    // es
    "alerg*", "enfermedad*", "enfermo", "enferma", "medico", "embaraz*", "asma", "vacun*",
];

/// Words that say health only beside another health word: alone they are as often something else (« traitement de
/// texte », « diagnostic du serveur », « handicap au golf », « signe : Cancer », « symptôme d'un bug », « santé
/// financière »). Alex's Carnet, 10/11: a question on the 2027 presidential candidates filed in Santé.
const HEALTH_WEAK: &[&str] = &[
    // fr
    "traitement*", "diagnost*", "symptom*", "chirurg*", "opere", "operee", "cancer*", "depressi*", "therap*",
    "psycholog*", "blessure*", "blesse", "blessee", "douleur*", "sante", "handicap*", "enceinte",
    // en
    "medicine*", "treatment*", "sick", "diagnos*", "surgery", "depress*", "injur*", "physio*", "health",
    "disabilit*",
    // de
    "behandlung*", "klinik*", "krebs", "verletz*", "schmerz*", "gesundheit*", "behinder*",
    // es
    "tratamiento*", "cirug*", "dolor*", "salud",
];

/// Phrases that are never health, taken out before the words are looked for.
const NOT_HEALTH: &[&str] = &[
    "traitement de texte", "traitement des donnees", "traitement de donnees", "traitement de l information",
    "traitement mediatique", "diagnostic de performance energetique", "diagnostic energetique", "diagnostic immobilier",
    "enceinte bluetooth", "enceinte connectee", "enceinte de la", "enceinte de l", "opere par", "operee par",
    "sante financiere", "sante economique", "signe astrologique", "word processing", "data processing", "health check",
];

/// The different words (or phrases) of padded folded `padded` (« ␣mots␣ ») that `list` holds: a stem `x*` matches a
/// word starting with it, an entry without a star the word or phrase itself. Two stems matching the same word count
/// once (« diagnostic »: diagnost* and diagnos*).
fn hits(padded: &str, list: &[&str]) -> usize {
    let mut found: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in list {
        match entry.strip_suffix('*') {
            Some(stem) => {
                for word in padded.split(' ').filter(|w| !w.is_empty() && w.starts_with(stem)) {
                    found.insert(word.to_string());
                }
            }
            None => {
                if padded.contains(&format!(" {entry} ")) {
                    found.insert((*entry).to_string());
                }
            }
        }
    }
    found.len()
}

/// Money words (folded), the same way; with `news::mentions_money` (an amount next to a currency).
const MONEY: &[&str] = &[
    "argent", "budget*", "salaire*", "loyer*", "credit*", "pret", "prets", "impot*", "banque*", "epargne*", "dette*",
    "facture*", "money", "salary", "rent", "loan*", "tax", "taxes", "bank*", "savings", "debt*", "invoice*",
    "mortgage*", "geld", "gehalt*", "miete*", "kredit*", "steuer*", "ersparn*", "schulden", "rechnung*", "dinero",
    "presupuesto*", "sueldo*", "salario*", "alquiler*", "prestamo*", "impuesto*", "banco*", "ahorro*", "deuda*",
    "factura*", "hipoteca*", "finance*",
];

/// Whether folded `text` holds one of `list`'s entries (a stem `x*` at a word's start, else the word or phrase).
fn holds(text: &str, list: &[&str]) -> bool {
    hits(&format!(" {} ", fold(text)), list) > 0
}

/// A health fact (body, care, treatments, illnesses, pregnancy…): wherever it would be written, it goes to the
/// private health part (or « À ranger » without a health agent), never to the shared part. Strict: a word of
/// `HEALTH`, or two of `HEALTH_WEAK`, once the phrases of `NOT_HEALTH` are taken out.
pub fn is_health(text: &str) -> bool {
    let mut padded = format!(" {} ", fold(text));
    for phrase in NOT_HEALTH {
        padded = padded.replace(&format!(" {phrase} "), " ");
    }
    hits(&padded, HEALTH) > 0
        || hits(&padded, HEALTH_WEAK) >= 2
        || split_label(text).and_then(|(l, _)| label_of(&l)).is_some_and(|l| l.health)
}

/// Whether a text might touch health, for what is only held back (the phone's offline copy, the week's review, the
/// questions, « Toujours en tête »): any health word, even one that alone is often something else. Filing uses the
/// strict `is_health`.
pub fn touches_health(text: &str) -> bool {
    is_health(text) || holds(text, HEALTH_WEAK)
}

/// A money fact (budget, salary, rent, loans…, or an amount).
pub fn is_money(text: &str) -> bool {
    holds(text, MONEY) || super::news::mentions_money(text)
}

/// The label a (folded or not) label text names: one of `LABELS` by its names in any language or its words.
pub fn label_of(text: &str) -> Option<&'static Label> {
    let folded = fold(text);
    if folded.is_empty() {
        return None;
    }
    LABELS.iter().find(|l| l.names.iter().any(|n| fold(n) == folded) || l.words.contains(&folded.as_str()))
}

/// A label's name in the Carnet's language.
pub fn name_of(label: &Label, lang: &str) -> &'static str {
    label.names[super::lang_index(lang).min(3)]
}

/// « Pointure : 36 » → (« Pointure », « 36 »): a short label (4 words, 30 characters at most, no digit) before the
/// first colon, a value after it.
pub fn split_label(fact: &str) -> Option<(String, String)> {
    let (label, value) = fact.split_once(':')?;
    let (label, value) = (label.trim(), value.trim());
    let fine = !label.is_empty()
        && !value.is_empty()
        && label.chars().count() <= 30
        && label.split_whitespace().count() <= 4
        && !label.chars().any(|c| c.is_ascii_digit())
        && !label.contains("[[")
        && !label.contains("http");
    fine.then(|| (label.to_string(), value.to_string()))
}

/// The key a fact's label is compared by: the table's id when it is one of `LABELS`, else the folded label.
pub fn label_key(label: &str) -> String {
    label_of(label).map(|l| l.id.to_string()).unwrap_or_else(|| fold(label))
}

/// A fact put in the « Libellé : valeur » form when it says a label of the table in words: « Chausse du 36 » →
/// « Pointure : 36 », « Pointure 36 » → « Pointure : 36 ». Anything else as it was.
pub fn reshape(fact: &str, lang: &str) -> String {
    let fact = fact.trim();
    if split_label(fact).is_some() {
        return fact.to_string();
    }
    let words: Vec<&str> = fact.split_whitespace().collect();
    if words.len() > 8 {
        return fact.to_string();
    }
    // only the labels of a measure or a date (« Pointure », « Anniversaire »…): a job or a city said in words stays as
    // it is said, where the outline files it
    for l in LABELS.iter().filter(|l| !l.joins.is_empty()) {
        let mut starts: Vec<String> = l.joins.iter().map(|j| j.to_string()).collect();
        starts.extend(l.names.iter().map(|n| fold(n)));
        starts.sort_by_key(|s| std::cmp::Reverse(s.split(' ').count()));
        for start in &starts {
            let n = start.split(' ').count();
            if words.len() <= n || fold(&words[..n].join(" ")) != *start {
                continue;
            }
            let value = words[n..].join(" ");
            let value = value.trim_start_matches([':', '-', '–']).trim();
            // a value, not a sentence going on (« Taille la haie samedi » is not a height)
            let short = value.split_whitespace().count() <= 4;
            let measured = value.chars().any(|c| c.is_ascii_digit()) || l.joins.contains(&start.as_str());
            if !value.is_empty() && short && measured {
                return format!("{} : {value}", name_of(l, lang));
            }
        }
    }
    fact.to_string()
}

/// The words of a question to look for (folded, the stop words left out), and the labels it asks about (« chausse
/// du combien » → the shoe size).
pub fn query(text: &str) -> (Vec<String>, Vec<&'static str>) {
    let mut folded = format!(" {} ", fold(text));
    let mut labels: Vec<&'static str> = Vec::new();
    // the longest phrases first: « taille de chaussures » before « taille »
    let mut phrases: Vec<(&'static str, &'static str)> = Vec::new();
    for l in LABELS {
        for w in l.words.iter().chain(l.joins.iter()) {
            phrases.push((w, l.id));
        }
    }
    phrases.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
    for (phrase, id) in phrases {
        let padded = format!(" {phrase} ");
        if folded.contains(&padded) {
            folded = folded.replace(&padded, " ");
            if !labels.contains(&id) {
                labels.push(id);
            }
        }
    }
    let words = folded.split_whitespace().filter(|w| !STOP.contains(w)).map(str::to_string).take(8).collect();
    (words, labels)
}

/// An ID number in a fact: never kept (id_numbers.rs). An e-mail address, a site's login, a date or a phone number
/// is not one.
pub use super::id_numbers::id_number;


/// An e-mail address, lower case (None: not one).
pub fn email(text: &str) -> Option<String> {
    let t = text.trim().trim_matches(['<', '>']).to_lowercase();
    let (user, domain) = t.split_once('@')?;
    let fine = !user.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !t.chars().any(char::is_whitespace);
    fine.then_some(t)
}

/// A phone number in E.164 (« +33612345678 »), national numbers by the Carnet's language (fr +33, de +49, es +34, en
/// +44). None: not a phone number.
pub fn phone(text: &str, lang: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || " +.-()/".contains(c)) {
        return None;
    }
    let digits: String = t.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 8 || digits.len() > 15 {
        return None;
    }
    if t.starts_with('+') {
        return Some(format!("+{digits}"));
    }
    if let Some(rest) = digits.strip_prefix("00") {
        return Some(format!("+{rest}"));
    }
    let national = |code: &str, len: std::ops::RangeInclusive<usize>| {
        (digits.starts_with('0') && len.contains(&digits.len())).then(|| format!("+{code}{}", &digits[1..]))
    };
    match lang {
        "fr" => national("33", 10..=10),
        "de" => national("49", 10..=12),
        "en" => national("44", 11..=11),
        "es" => (digits.len() == 9 && digits.starts_with(['6', '7', '8', '9'])).then(|| format!("+34{digits}")),
        _ => None,
    }
}

/// The words that file a page in a category (plan « Le sommaire » §2.2, categories.rs), folded, in fr, en, de, es and
/// nl: the words of the apps' icon table (HermesKit `NotebookIcon`, scripts/notebook-icons.py) copied and extended, the
/// bridge now the reference. Each entry: its categories (the first the main one, a second one counting a little
/// less) and its stems: `x` the start of a word, `x=` the whole word. The longest stem a word matches decides it.
/// The last entry is « Autres » on purpose (hobbies, books, series, sport, learning: carnet-categories, 22/11): a page
/// it decides stays in « Autres » whatever its first line says.
/// Health words file nothing here: a line of the shared Carnet that looks like health stays in its page's category
/// (Alex's decision 6, 21/11); Santé is the private part. The papers that go with an illness do file their page in
/// Papiers (« arrêt maladie », « feuille de soins », « sick note »: carnet-categories).
pub const CATEGORY_WORDS: &[(&[&str], &[&str])] = &[
    // trips
    (&["trips"], &[
        "voyag", "vacanc", "avion", "vol=", "vols=", "aeroport", "sejour", "trip", "travel", "holiday", "vacation",
        "flight", "airport", "reise", "urlaub", "flug", "ferien", "viaj", "vacacion", "vuelo", "aeropuerto", "reis=",
        "reizen", "reisbudget", "reisje", "vakantie", "vlucht", "vliegveld", "luchthaven", "japon", "japan", "italie",
        "italy", "italien", "italia", "espagne", "spain", "spanien", "portugal", "grece", "greece", "griechenland",
        "grecia", "maroc", "morocco", "marokko", "marruecos", "mexique", "mexico", "canada", "kanada", "bali=",
        "thailande", "thailand", "tailandia", "vietnam", "islande", "iceland", "island=", "islandia", "ijsland",
        "ecosse", "scotland", "schottland", "escocia", "schotland", "irlande", "ireland", "irland", "irlanda",
        "ierland", "londres", "london", "londen", "rome=", "rom=", "roma=", "barcelone", "barcelona", "tokyo", "tokio",
        "kyoto", "kioto", "osaka", "lisbonne", "lisbon", "lissabon", "lisboa", "weekend", "hotel", "airbnb",
        "croisiere", "cruise", "kreuzfahrt", "crucero", "valise", "suitcase", "koffer", "maleta", "passagier",
        "billet d avion", "camping", "randonn", "hiking", "wander", "senderismo",
        // carnet-categories (22/11)
        "train=", "trains=", "tgv=", "sncf", "gare=", "billet", "billete", "ski=", "skis=", "skier", "skiing",
        "skipass", "skipas", "skiurlaub", "skifahren", "esqui", "wintersport", "lift pass", "lift passes",
        "forfait de ski", "forfaits de ski", "forfait ski", "forfait de esqui", "gite", "chambre d hote",
        "chambres d hotes", "auberge", "location de vacances", "location de voiture", "voiture de location",
        "road trip", "roadtrip", "escapade", "excursion", "plage", "creme solaire", "bagage", "itineraire", "escale",
        "croisi", "week end", "rail=", "railcard", "eurostar", "ferry", "packing", "pack list", "luggage", "baggage",
        "holiday cottage", "holiday let", "car hire", "hire car", "hire a car", "rental car", "city break", "getaway",
        "staycation", "campsite", "caravan", "sightseeing", "itinerary", "jet lag", "beach", "abroad", "honeymoon",
        "hostel", "zug=", "zugfahrt", "bahnfahrt", "bahnreise", "packliste", "gepack",
        "ferienwohnung", "hotelzimmer", "ferienhaus", "mietwagen", "rundreise", "ausflug", "strand", "kurztrip",
        "tren=", "trenes=",
        "equipaje", "casa rural", "alquiler de coche", "coche de alquiler", "playa", "escapada", "ruta por", "hostal",
        "albergue", "trein", "paklijst", "huurauto", "vakantiehuis", "vakantiewoning", "rondreis", "stedentrip",
        "citytrip", "dagje uit", "bretagne", "normandie", "corse", "provence", "venise", "venice", "venedig",
        "venecia", "venetie", "crete=", "kreta", "creta", "sicile", "sicily", "sizilien", "sicilia", "croatie",
        "croatia", "kroatien", "croacia", "kroatie", "egypte", "egypt", "agypten", "egipto", "norvege", "norway",
        "norwegen", "noruega", "noorwegen", "edimbourg", "edinburgh", "edimburgo", "porto=", "oporto", 
        "cornwall",
        "cornouailles", "algarve", "andalous", "andalusia", "andalucia", "tenerif", "majorque", "mallorca", "ibiza",
        "canaries", "kanaren", "canarias", "toscane", "tuscany", "toskana", "toscana", "new york", 
        "montagne", "mer=", "etranger", "mountain", "seaside", "berge=", "ausland", "montana",
        "extranjero", "buitenland", "uitstapje",
    ]),
    // home: the house, works, furniture, garden, pets, energy, cleaning, cooking, the building
    (&["home"], &[
        "maison", "home=", "house", "haus=", "hause", "zuhause", "casa=", "hogar", "foyer", "domicile", "domotique",
        "huis=", "huishoud", "thuis", "woning", "appart", "apartment", "flat=", "wohnung", "piso=", "departamento",
        "loyer", "rent=", "miete", "alquiler", "proprio=", "landlord", "vermiet", "coloc", "roommate", "huur",
        "verhuur", "demenag", "moving", "umzug", "mudanza", "emmenag", "verhuiz", "immobil", "immo=", "inmobil",
        "propriet", "property", "eigentum", "propiedad", "terrain", "grundstuck", "terreno", "vastgoed", "renovation",
        "renover", "renov", "travaux", "chantier", "artisan", "plomb", "electricien", "peintur", "carrel", "isolation",
        "toiture", "toit=", "remodel", "repair", "reparat", "umbau", "sanier", "renovier", "handwerk", "reforma",
        "obra=", "obras=", "verbouw", "renovatie", "klus", "aannemer", "loodgieter", "kitchen", "kuche", "keuken",
        "bathroom", "badezimmer", "bano=", "badkamer", "sdb=", "devis", "offerte", "bricol", "diy=", "outil", "tool=",
        "tools=", "heimwerk", "werkzeug", "bricolaje", "herramienta", "gereedschap", "meuble", "furniture", "mobel",
        "mueble", "meubel", "ikea", "jardin", "garden", "garten", "plante", "pflanz", "planta", "potager",
        "pelouse", "lawn", "rasen", "cesped", "tuin", "gazon", "arbre", "tree=", "baum", "arbol", "compost", "balcon",
        "terrasse", "chien", "dog=", "dogs=", "cat=", "cats=", "hund", "katze", "perro", "gato", "hond=", "honden=",
        "kat=", "katten", "veterin", "vet=", "dierenarts", "haustier", "mascota", "huisdier", "chiot", "puppy",
        "chaton", "kitten", "croquette", "menage", "cleaning", "putz", "limpieza", "schoonmaak", "nettoy", "lessive",
        "laundry", "wasche", "colada", "energ", "electricite", "electric", "strom", "stroom", "chauffage", "heating",
        "heizung", "calefaccion", "verwarming", "chaudiere", "boiler", "caldera", "ketel", "solaire", "solar",
        "compteur", "thermostat", "climatis", "klima", "cuisine", "cuisiner", "recette", "recipe", "rezept", "receta",
        "recept=", "recepten=", "cocina", "cocinar", "kochen", "koken", "repas", "meal", "maaltijd",
        "patisser", "baking", "backen", "hornear", "syndic", "copro", "hausverwaltung", "administrador de fincas",
        "comunidad de vecinos", "vve=", "poubelle", "bin=", "basura", "vuilnis", "cle=", "cles=", "key=", "keys=",
        "schlussel", "llave", "sleutel",
        // carnet-categories (22/11)
        "tondre", "tondeuse", "haie=", "haies=", "taille haie", "jardinage", "arrosage", "arroser", "robinet", "evier",
        "fuite=", "fuites=", "radiateur", "chauffe eau", "repeindre", "peindre", "papier peint", "tapisserie",
        "parquet", "moquette", "cave=", "caves=", "grenier", "cartons=", "desencombr", "vide grenier",
        "encombrants", "dechetterie", "electromenager", "lave linge", "lave vaisselle", "machine a laver", "frigo",
        "refrigerateur", "congelateur", "aspirateur", "canape", "matelas", "literie", "rideau", "volets=", "fenetre",
        "vitres=", "serrure", "serrurier", "menuisier", "charpentier", "couvreur", "gouttiere", "facade", "ravalement",
        "bailleur", "locataire", "bail=", "etat des lieux", "pompe a chaleur", "litiere", "veto=", "repassage",
        "perceuse", "etagere", "plumber", "plumbing", "leak=", "leaks=", "leaking", "sink=", "tap=", "taps=",
        "radiator", "mow=", "mowing", "mower", "hedge", "weeding", "paint=", "wallpaper", "carpet", "flooring",
        "loft=", "attic", "cellar", "basement", "shed=", "declutter", "clear out", "dishwasher", "washing machine",
        "fridge", "freezer", "hoover", "vacuum", "sofa", "mattress", "curtain", "locksmith", "roofer", "gutter",
        "carpenter", "joiner", "electrician", "builder", "tenant", "tenancy", "letting agent", "estate agent",
        "recycling", "spring clean", "pet=", "pets=", "hamster", "heat pump", "insulation", "spare room", "klempner",
        "wasserhahn", "abfluss", "heizkorper", "mahen", "hecke", "unkraut", "streichen", "tapete", "tapezier",
        "keller", "dachboden", "ausmisten", "entrumpel", "sperrmull", "wertstoff", "waschmaschine", "spulmaschine",
        "geschirrspul", "kuhlschrank", "staubsaug", "matratze", "vorhang", "fenster", "schlusseldienst", "dachdecker",
        "tischler", "schreiner", "elektriker", "mieter", "nebenkosten", "mietkaution", "hausmeister", "eigentumer",
        "fruhjahrsputz", "tierarzt", "photovoltaik", "warmepumpe", "dammung", "zimmer=", "kinderzimmer",
        "schlafzimmer", "wohnzimmer", "gastezimmer", "fontaner", "fuga=", "grifo",
        "fregadero", "radiador", "segar", "seto=", "pintar", "pintor", "habitacion", "trastero", "desvan", "garaje",
        "sotano", "lavadora", "lavavajillas", "nevera", "frigorifico", "aspiradora", "colchon", "cortina", "ventana",
        "cerrajer", "carpinter", "electricista", "albanil", "casero=", "inquilin", "junta de vecinos", 
        "lekkage", "lekkend", "lek=", "kraan", "gootsteen", "cv ketel", "maaien", "heg=", "heggen", "schilder",
        "verf=", "zolder", "kelder",
        "schuur", "milieustraat", "afval", "wasmachine", "vaatwasser", "koelkast", "stofzuig", "matras", "gordijn",
        "slotenmaker", "timmerman", "elektricien", "borg=", "bewoner", "zonnepan", "warmtepomp", "isolatie",
        "woonkamer", "slaapkamer", "plant=", "plants=", "planta=", "plantas=", "cook=", "cooking", "cookery",
        "logement", "salle de bain", "linge=", "ascenseur", "animaux=", "animal=", "bedroom", "living room",
        "decorating", "chores", "bins=", "gas=", "haushalt", "mull=", "hausordnung", "dormitorio", "electrodomestic",
        "fianza",
    ]),
    // money
    (&["money"], &[
        "budget", "depense", "epargne", "saving", "expense", "ausgab", "sparen", "gasto", "ahorro", "presupuesto",
        "uitgave", "spaar", "financ", "finanz", "argent", "money", "geld", "dinero", "banque", "bank", "compte=",
        "comptes=", "credit", "pret=", "emprunt", "loan", "mortgage", "hypothe", "kredit", "banco", "prestamo",
        "hipoteca", "hypotheek", "lening", "livret", "retraite", "pension", "pensioen", "rente=", "invest",
        "belegg", "bourse", "stock=", "aktie", "inversion", "crypto", "abonnement", "abonn", "subscription", "abo=",
        "suscrip", "forfait", "achat", "acheter", "buy=", "kauf", "einkauf", "compra", "comprar", "aankoop", "kopen",
        "courses=", "shopping", "boodschappen", "commande", "bestell", "pedido", "bestelling", "soldes", "salaire",
        "salary", "gehalt", "sueldo", "salario", "salaris", "paie", "payslip", "lohn", "nomina", "loonstrook", "payer",
        "paiement", "payment", "zahlung", "pago", "betaling", "refund", "erstattung", "reembolso",
        "terugbetaling",
        // carnet-categories (22/11)
        "rembours", "dette", "dettes=", "preter=", "economies", "economiser", "mettre de cote", "virement", "cheque=",
        "cheques=",
        "carte bancaire", "carte bleue", "decouvert=", "agios", "frais bancaires", "releve de compte", "taux=",
        "assurance vie", "tirelire", "argent de poche", "prime=", "allocation", "mensualit", "echeancier",
        "prelevement", "netflix", "spotify", "disney=", "deezer", "black friday", "promo=", "promos=", "vinted",
        "leboncoin", "revendre", "spending", "spend=", "owe=", "owed", "debt", "lend=", "lent=", "pocket money",
        "allowance", "overdraft", "direct debit", "standing order", "interest rate", "remortgag", "membership",
        "purchase", "price=", "prices=", "cost=", "costs=", "afford", "payday", "wage=", "wages=", "cashback",
        "voucher", "haushaltsbuch", "haushaltskasse", "sparplan", "sparkont", "sparbuch", "sparkass", "erspar",
        "konto", "tagesgeld", "festgeld", "zinsen", "dispo=", "uberweis", "lastschrift", "dauerauftrag", "schulden",
        "zuruckzahl", "taschengeld", "kindergeld", "elterngeld", "weihnachtsgeld", "urlaubsgeld", "handyvertrag",
        "handytarif", "mobilfunk", "cuenta de ahorro", "cuenta corriente", "cuenta bancaria", "ahorrar", "deuda",
        "paga=", "pagar=", "tarifa",
        "transferencia", "bizum", "rebajas", "zakgeld", "rekening", "schuld=", "schulden=", "terugbetal",
        "terugstort", "overmaken", "betalen", "betaal", "oversluit", "toeslag", "uitverkoop", "korting",
        "kinderbijslag", "huishoudgeld", "jubilacion", "jubilad", "banquier", "pel=", "lep=", "bitcoin",
        "impaye", "rib=", "stocks=", "shares=", "paypal", "einnahmen", "kosten=", "etf=", "ingresos", "inkomsten",
        "aandelen", "loon=",
    ]),
    // taxes: money, and the papers that go with them
    (&["money", "papers"], &[
        "impot", "taxe", "tax=", "taxes=", "fiscal", "steuer", "finanzamt", "impuesto", "hacienda", "belasting",
        "aangifte", "declaration d impot",
        // carnet-categories (22/11)
        "taxe fonciere", "taxe d habitation", "foyer fiscal", "council tax", "grundsteuer", "ibi=", "renta=",
        "declaracion de la renta",
    ]),
    // bills: papers (« Factures »), and money
    (&["papers", "money"], &[
        "factur", "invoice", "bill=", "bills=", "rechnung", "recibo", "quittance", "factuur", "facturen", "nota=",
        // carnet-categories (22/11)
        "utility bill", "energierekening", "stromrechnung", "mahnung",
    ]),
    // papers: admin, contracts, insurance, guarantees, letters
    (&["papers"], &[
        "admin", "papier", "paperasse", "paperwork", "document", "dossier", "formulaire", "formular", "formulario",
        "formulier", "passeport", "passport", "reisepass", "pasaporte", "paspoort", "identite", "identity", "ausweis",
        "identidad", "identiteit", "visa=", "caf=", "secu=", "prefecture", "mairie", "behord", "amt=", "tramite",
        "papeles", "papieren", "gemeente", "vergunning", "certificat", "certificate", "zertifikat", "certificado",
        "attestation", "bescheinigung", "juridique", "legal", "lawyer", "anwalt", "abogado", "advocaat", "notaire",
        "notary", "notar", "notaris", "contrat", "contract", "vertrag", "contrato", "litige", "dispute", "streit",
        "tribunal", "gericht", "rechtbank", "plainte", "complaint", "succession", "heritage", "inheritance", "erbe=",
        "erbschaft", "herencia", "erfenis", "testament", "divorce", "scheidung", "divorcio", "scheiding", "assur",
        "mutuelle", "insurance", "insur", "versicher", "seguro", "verzeker", "garantie", "warranty", "garantia",
        "lettre", "letter", "carta=", "courrier", "permis", "licence", "license", "fuhrerschein", "rijbewijs",
        "carnet de conducir",
        // carnet-categories (22/11)
        "carte grise", "carte d identite", "cni=", "titre de sejour", "procuration", "arret maladie",
        "arret de travail", "feuille de soins", "feuilles de soins", "carte vitale", "ameli=", "cpam=", "resili",
        "preavis", "acte de naissance", "acte de deces", "livret de famille", "libro de familia", "livret scolaire",
        "etat civil", "justificatif", "justificatif de domicile", "inscription", "commissariat", "amende",
        "controle technique", "sav=", "ticket de caisse", "form=", "forms=", "admission", "enrol",
        "registration", "register=", "solicitor", "power of attorney", "probate", "dvla", "parking permit",
        "parking fine", "policy=", "policies=", "cancel=", "cancellation", "guarantee", "receipt", "sick note",
        "fit note", "anmeldung",
        "ummeldung", "abmeldung", "einwohnermeldeamt", "burgeramt", "standesamt", "urkunde", "unterlagen", "antrag=",
        "antrage=",
        "antrage=", "krankmeld", "krankschreib", "gelber schein", "kundig", "police=", "zulassung", "tuv=",
        "hauptuntersuchung", "vollmacht", "kassenbon", "quittung", "cita previa", "comisaria", "ayuntamiento",
        "empadron", "padron", "registro civil", "partida de nacimiento", "matricula", "solicitud", "instancia",
        "baja medica", "baja laboral", "darse de baja", "parte de baja", "poliza", "multa", "itv=", "dni=", "pasfoto",
        "apk=", "kenteken", "polis=",
        "ziekmeld", "uittreksel", "geboorteakte", "akte=", "aanvraag", "aanvragen", "inschrijv", "opzeg", "bonnetje",
        "kassabon", "boete", "huissier", "naturalisation", "deed=", "deeds=", "id=", "seguridad social",
        "permiso", "licencia", "digid",
    ]),
    // work
    (&["work"], &[
        "travail", "boulot", "job=", "jobs=", "work=", "arbeit", "trabajo", "werk=", "baan=", "emploi", "carriere",
        "career", "karriere", "bureau", "office", "buro", "oficina", "kantoor", "collegue", "colleague", "kollege",
        "colega", "collega", "reunion d equipe", "reunion de travail", "meeting", "besprech", "vergader", "interview",
        "cv=", "lebenslauf", "candidat", "bewerb", "sollicit", "recrut", "recruit", "chomage", "boss", "patron",
        "baas=", "manager", "teletravail", "homeoffice", "thuiswerk", "client", "customer", "kunde", "klant",
        "prospect", "vertrieb", "commercial", "entreprise", "societe", "company", "startup", "firma", "empresa",
        "unternehmen", "bedrijf", "onderneming", "sarl", "kbis", "freelance", "zzp", "business", "negocio", "comptab",
        "accounting", "buchhalt", "contabil", "boekhoud", "entretien annuel", "annual review", "jahresgesprach",
        "entrevista anual", "jaargesprek", "formation", "schulung", "formacion", "opleiding", "poste=", "stelle=",
        "puesto=",
        // carnet-categories (22/11)
        "embauche", "convention de stage", "stage de 3e", "stage de fin d etudes", "rapport de stage", "stagiaire",
        "alternance", "lettre de motivation", "pot de depart",
        "notes de frais", "note de frais", "frais de deplacement", "seminaire", "rapport=", "visio=",
        "rtt=", "conges payes", "fiche de poste", "demission", "licenciement", "rupture conventionnelle",
        "france travail", "pole emploi", "auto entrepreneur", "micro entreprise", "linkedin", "internship", "intern=",
        "apprenticeship", "cover letter", "cover letters", "pay rise", "payrise", "appraisal", "performance review",
        "quarterly", "shift=", "shifts=", "night shift", "on call", "overtime", "timesheet", "expenses claim",
        "expense claim", "leaving drinks", "leaving do", "leaving party", "conference", "trade fair", "offsite",
        "away day", "onboarding", "deadline", "hr=", "payroll", "redundancy", "resign", "headhunter", "management",
        "working from home", "work from home", "wfh=", "vorstellungsgesprach", "praktikum", "ausbildung", "quartal",
        "reisekosten", "spesen", "dienstreise", "geschaftsreise", "schicht", "dienstplan", "nachtdienst",
        "bereitschaft", "uberstunden", "gehaltserhoh", "beforderung", "arbeitgeber", "arbeitnehmer", "chef=", "chefin",
        "weiterbildung", "fortbildung", "betriebsrat", "auftrag", "entrevista", "curriculum", "practicas", "becari",
        "informe", "trimestral", "turno de noche", "turnos de noche", "cambio de turno", "guardia", "horas extra",
        "subida de sueldo",
        "aumento de sueldo", "jefa=", "jefe=", "congreso", "autonom", "vacature", "stagiair", "kwartaal", "rooster",
        "overuren", "salarisverhoging", "loonsverhoging", "declaratie reiskosten", "reiskosten", "werkgever",
        "werknemer",
        "werkplek", "werkoverleg", "leidinggevende", "functioneringsgesprek", "beoordelingsgesprek", "mutation",
        "mission=", "astreinte", "heures sup", "heures supplementaires", "commute", "staff=", "personalgesprach",
        "mitarbeitergesprach", "teletrabajo", "paro=", "despido", "ontslag", "werkloos", 
    ]),
    // people: the person's own people, their dates, gifts and parties
    (&["people"], &[
        "famil", "gezin", "parent", "eltern", "padres", "ouders", "maman", "papa", "mama", "mom=", "dad=", "mere=",
        "pere=", "mutter", "vater", "madre", "padre", "moeder", "vader", "mamie", "papi=", "grandma", "grandpa",
        "oma=", "opa=", "abuel", "grootouder", "cousin", "tante", "oncle", "uncle", "aunt", "frere", "soeur",
        "brother", "sister", "bruder", "schwester", "hermano", "hermana", "broer", "zus=", "zussen", "oom=", "neef",
        "ami=", "amis=", "amie=", "amies=", "copain", "copine", "pote=", "potes=", "friend", "freund",
        "amigo", "amiga", "vriend", "voisin", "neighbo", "nachbar", "vecino", "buren", "buurman", "buurvrouw",
        "enfant", "child", "kid=", "kids=", "kinder", "kinderen", "nounou", "creche", "kita", "babysit", "garderie",
        "nanny", "oppas", "hijo", "hija", "fils=", "fille=", "bebe", "baby", "naissance", "birth", "geburt",
        "nacimiento", "geboorte", "anniv", "birthday", "geburtstag", "cumple", "verjaardag", "jarig", "mariage",
        "marier", "wedding", "marry", "hochzeit", "heirat", "boda", "casamiento", "bruiloft", "huwelijk", "trouw",
        "pacs=", "fiancail", "verlob", "temoin", "cadeau", "gift", "geschenk", "regalo", "wishlist", "fete", "party",
        "soiree", "feier", "fiesta", "feest", "celebr", "apero", "barbecue", "bbq=", "invite", "guest", "gast=",
        "gaste=", "invitad", "noel", "christmas", "weihnacht", "navidad", "kerst", "reveillon", "ehpad", "elderly",
        "aidant", "caregiver", "residencia", "mantelzorg",
        // carnet-categories (22/11)
        "mum=", "mums=", "mummy", "papy", "grand mere", "grand pere", "grands parents", "petit fils", "petite fille",
        "petits enfants", "tonton", "tata=", "neveu", "niece", "parrain", "marraine", "filleul", "beau frere",
        "belle soeur", "belle mere", "beau pere", "conjoint", "mari=", "obseques", "enterrement", "condoleances",
        "bapteme", "communion", "maison de retraite", "arbre genealogique", "repas de famille", "ecole=", "maternelle",
        "cantine", "kermesse", "sortie scolaire", "fournitures scolaires", "maitresse", "nan=", "nana=",
        "grandad", "granddad", "grandmother", "grandfather", "grandparent", "grandchild", "grandson", "granddaughter",
        "nephew", "godson", "goddaughter", "godparent", "godmother", "godfather", "in laws", "husband", "wife=",
        "childminder", "school run", "teacher", "funeral", "christening", "baptism", "anniversary", "sleepover",
        "playdate", "care home", "carer", "omas=", "opas=", "grosseltern", "enkelin", "enkelkind", "neffe",
        "nichte=", "patenkind", "schwager", "schwagerin", "schwieger", "ehemann", "ehefrau", "kindergarten",
        "tagesmutter", "beerdigung", "trauerfeier", "taufe", "konfirmation", "kommunion", "trauzeug", "jubilaum",
        "pflegeheim", "altenheim", "seniorenheim", "einschulung", "klassenfahrt", "grillabend", "grillparty",
        "grillen", "nieto", "nieta", "sobrino", "sobrina", "primo=", "primos=", "tio=", "tia=", "tios=", "padrino",
        "madrina", "ahijad", "suegro", "suegra", "cunado", "cunada", "marido", "esposa", "esposo", "novio", "novia",
        "canguro", "ninera", "guarderia", "colegio", "bautizo", "entierro", "velatorio", "kleinkind", "kleinzoon",
        "kleindochter", "nichtje", "peetoom", "peettante", "schoonmoeder", "schoonvader", "schoonouder", "zwager",
        "schoonzus", "echtgeno", "kinderopvang", "ouderavond", "oudergesprek", "begrafenis", "crematie", "uitvaart",
        "doop=", "getuige", "verzorgingshuis", "verpleeghuis", "bejaardentehuis", "logeer", "deces", "deuil",
        "garde d enfants", "garde alternee", "rentree scolaire", "playgroup", "daughter", "grandkid", "toddler",
        "teenager", "fiance", "relatives=", "bereavement", "sohn", "tochter", "onkel", "lehrer", "trauer", "profesor",
        "maestr", "zoon=", "dochter", "juf=",
    ]),
    // carnet-categories (22/11): what a person keeps in « Autres » — hobbies, books, films and series, music, sport,
    // learning — so that a word of its first line does not pull it elsewhere (the page stays in « Autres »)
    (&["other"], &[
        "livres=", "lecture=", "lectures=", "roman=", "romans=", "polar", "bd=", "film=", "films=",
        "series=", "cinema", "theatre", "concert", "musique", "guitare", "piano", "violon", "chorale", "chant=",
        "danse", "dessin", "photo=", "photos=", "tricot", "crochet", "couture", "broderie", "jeux=", "puzzle",
        "echecs", "sport=", "sports=", "foot=", "football", "tennis", "rugby", "natation", "piscine", "yoga",
        "pilates", "course a pied", "running", "marathon", "velo=", "peche=", "apprendre", "espagnol", "anglais",
        "allemand", "duolingo", "podcast", "sortie=", "sorties=", "expo=", "exposition", "musee", "escape game",
        "loisir", "books=", "reading list", "novel=", "novels=", "thriller", "movie", "shows=", "tv=", "music",
        "guitar",
        "violin", "choir", "singing", "dancing", "drawing", "photography", "knitting", "sewing", "embroidery", "chess",
        "board game", "gaming", "golf=", "cricket", "swimming", "parkrun", "cycling", "fishing", "learning", "spanish",
        "french=", "german=", "exhibition", "museum", "gig=", "gigs=", "hobby", "hobbies", "buch=", "bucher=",
        "lesen=", "krimi", "filme=", "serie=", "serien=", "kino", "musik", "gitarre", "klavier", "geige", "chor=",
        "chorprobe", "singen", "tanzen", "zeichnen", "fotograf=", "fotografin", "stricken", "hakeln", "nahen=",
        "schach=", "schachclub", "schachverein", "spiele=",
        "fussball", "schwimmen", "laufen=", "radfahren", "angeln", "lernen", "spanisch", "englisch", "sprachkurs",
        "ausstellung", "konzert", "theater", "libro=", "libros=", "leer=", "novela", "pelicula", "cine=", "musica",
        "guitarra", "coro=", "ensayo", "cantar", "baile", "dibujo", "fotografia", "ganchillo", "costura", "ajedrez",
        "juegos", "futbol", "tenis=", "padel", "natacion", "correr", "maraton", "bici=", "pesca=", "aprender",
        "ingles=", "frances=", "aleman=", "exposicion", "museo", "concierto", "teatro", "aficion", "boek=", "lezen=",
        "bioscoop", "muziek", "gitaar", "viool", "koor=", "koorrepetitie", "zingen", "dansen", "fotografie",
        "breien", "haken=", "naaien", "puzzel", "schaken", "spelletjes", "voetbal", "hockey", "zwemmen", "hardlopen",
        "fietsen", "vissen", "leren=", "spaans", "engels", "taalcursus", "tentoonstelling", "uitje=", "uitjes=",
        "benevol", "volunteer", "ehrenamt", "voluntari", "vrijwillig", "club=", "jeu video", "jeux video",
    ]),
    // carnet-categories, review 1 (22/11): phrases that file nothing, taken out before their words are read (« compte
    // rendu » is no account, « tener en cuenta » no bank, « planta baja » no paper)
    (&[], &[
        "compte rendu", "comptes rendus", "prelevement sanguin", "tener en cuenta", "darse cuenta", "planta baja",
        "temporada baja", "rekening houden", "in overleg", "im zuge", "last resort", "meter reading", "chore rota",
    ]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_facts_take_the_label_form() {
        assert_eq!(reshape("Chausse du 36", "fr"), "Pointure : 36");
        assert_eq!(reshape("Pointure 36", "fr"), "Pointure : 36");
        assert_eq!(reshape("Pointure : 37", "fr"), "Pointure : 37");
        assert_eq!(reshape("Née le 12 mars 1990", "fr"), "Anniversaire : 12 mars 1990");
        assert_eq!(reshape("Wears size 9", "en"), "Shoe size : 9");
        assert_eq!(reshape("Aime le jazz", "fr"), "Aime le jazz");
        assert_eq!(reshape("Taille la haie samedi avec son voisin", "fr"), "Taille la haie samedi avec son voisin");
        assert_eq!(label_key("Pointure"), "shoe_size");
        assert_eq!(label_key("Taille de chaussures"), "shoe_size");
        assert_eq!(label_key("Couleur du vélo"), "couleur du velo");
        assert!(split_label("Devis plombier accepté le 22/10 : 480 €").is_none());
        assert!(split_label("[[Léa]] : sœur").is_none());
    }

    #[test]
    fn a_question_gives_its_words_and_labels() {
        let (words, labels) = query("Léa chausse du combien ?");
        assert_eq!(words, ["lea"]);
        assert_eq!(labels, ["shoe_size"]);
        let (words, labels) = query("What is the shoe size of Léa");
        assert_eq!((words, labels), (vec!["lea".to_string()], vec!["shoe_size"]));
        assert_eq!(query("anniversaire de Marc").1, ["birthday"]);
    }

    #[test]
    fn health_and_money_words() {
        for fact in ["Allergique à la pénicilline", "Allergies : arachides", "Sous traitement pour la thyroïde",
                     "Enceinte de 4 mois", "Allergic to peanuts", "Hat Diabetes", "Alergia al polen", "Médecin : Dr Roux"] {
            assert!(is_health(fact), "{fact}");
        }
        for fact in ["Aime le jazz", "Pointure : 36", "Chat : Minou", "Pain au chocolat le dimanche", "Joue au tennis"] {
            assert!(!is_health(fact), "{fact}");
        }
        for fact in ["Kiné le mardi 18 h", "Opéré du genou en mars", "Dépression : suivie par une psychologue",
                     "Cancer du sein en 2019", "Douleur au dos, voir l'ostéopathe", "Vaccin grippe : fait le 12 oct.",
                     "Under treatment for asthma", "Ist schwanger", "Rendez-vous chez le dentiste de Zoé"] {
            assert!(is_health(fact), "{fact}");
        }
    }

    /// Alex's Carnet, 10/11: things that are not health, filed in Santé by one word each.
    #[test]
    fn words_that_are_often_something_else_are_not_health() {
        for fact in [
            "Question : qui sont les candidats à la présidentielle 2027 ?",
            "Comparer les programmes santé des candidats à la présidentielle 2027",
            "Présidentielle 2027 : candidats déclarés, traitement médiatique à suivre",
            "Diagnostic de performance énergétique (DPE) : classe D",
            "Diagnostic du serveur VPS-2 fait le 3/11",
            "Traitement de texte : préfère LibreOffice",
            "Traitement des données hébergé en UE (RGPD)",
            "Achète une enceinte Bluetooth pour le salon",
            "Vol opéré par Air France",
            "Handicap au golf : 18",
            "Signe astrologique : Cancer",
            "Psychologie du consommateur : étude livrée",
            "Frappe chirurgicale dans le débat",
            "Bonne santé financière de Kerfy Studio",
            "Symptôme d'un bug : l'app se ferme",
            "Healthcheck du guichet toutes les 5 min",
            "Douleur du client : trop de jetons",
            "Dépression tropicale sur la Réunion",
            "Thérapie de groupe pour l'équipe : non",
            "Health check of the server every night",
            "Krebs essen am Freitag",
            "Emma, cousine : infirmière à Lille",
        ] {
            assert!(!is_health(fact), "{fact}");
        }
        assert!(touches_health("Comparer les programmes santé des candidats"));
        assert!(is_money("Loyer : 950 €") && is_money("Budget vacances 2 000 €") && is_money("Rent is due"));
        assert!(!is_money("Rentrée le 2 septembre"));
    }

    #[test]
    fn id_numbers_are_caught_phones_and_dates_pass() {
        for fact in ["Passeport : 12AB34567", "Numéro de passeport 12 AB 34567", "IBAN FR76 3000 6000 0112 3456 7890 189",
                     "Carte 4970 1012 3456 7890", "Sécu : 1 85 05 78 006 084 36"] {
            assert!(id_number(fact), "{fact}");
        }
        for fact in ["Téléphone : 06 12 34 56 78", "Passeport valable jusqu'au 2031-03-12", "Passeport expire le 12/03/2031",
                     "Anniversaire : 14 novembre", "Code postal 69003", "+33 6 12 34 56 78"] {
            assert!(!id_number(fact), "{fact}");
        }
    }

    /// bridge-tools-no-breaker (14/11): the bench's notes were refused three times in a row (« le 14 novembre 2026
    /// sur la Pharmacie » read as an IBAN once the spaces were squeezed out), then Hermes paused all the bridge's tools.
    #[test]
    fn emails_logins_and_dates_are_not_id_numbers() {
        for fact in [
            // the bench's notes (14/11, DeepSeek), as the agent wrote them
            "Compte client créé le 14 novembre 2026 sur la Pharmacie des Essais, pour commander en ligne.",
            "Compte client créé le 14 novembre 2026 (pharmacie en ligne, 127.0.0.1:8088)",
            "Pharmacie des Essais (127.0.0.1:8088) : compte client créé le 14/11/2026 (Camille Testeur, \
camille.testeur@example.org, Mme, née le 14/03/1991) — identifiant et mot de passe dans le coffre",
            "Identifiant : camille.testeur@example.org — mot de passe gardé dans le coffre",
            // e-mail addresses, digits and ID-like words in them included
            "Login: jdupont1985@gmail.com",
            "E-Mail: max.mustermann1990@web.de",
            "Usuario: maria.garcia19900101@correo.es",
            "Adresse du service passeport : passeport.service2024@mairie-exemple.fr",
            "Contacto NIE: nie.extranjeros2024@policia.example.es",
            "Identifiant : fr76jeanmartin1234@example.org",
            "Compte : 4970101234567890@cartes.example.com",
            // a site's login identifier
            "Identifiant client : 4500123987",
            "Login : camille_t92",
            "Benutzername: max.mustermann_1990",
            "Usuario: maria_garcia_1990",
            "Numéro client : CL-2026-118842",
            "Espace client https://www.exemple.fr/compte/12345678901234",
            // dates, in the four languages, next to an ID word too
            "Rendez-vous le 14 novembre à la mairie pour le passeport",
            "Passport renewed on 14 March 2026",
            "Passport renewed on March 14, 2026",
            "Reisepass beantragt am 14.03.2026",
            "Pasaporte renovado el 14/03/2026",
            "Passeport valable jusqu'au 12-03-2031",
            "Lu 12 fois avec Paul dans le train",
            "Vu le 12 mars avec Léa au cinéma, très bien",
            // phone numbers (a German mobile has 13 digits)
            "Telefon: +49 151 23456789",
            "Telefon: 0049 151 23456789",
            "Móvil: +34 612 345 678",
            "Mobile: +44 7700 900123",
            // « nie » is German for « never »
            "Er ruft nie zurück, Kundennummer 123456",
        ] {
            assert!(!id_number(fact), "{fact}");
        }
    }

    #[test]
    fn id_numbers_in_the_four_languages_are_refused() {
        for fact in [
            // fr
            "Passeport : 12AB34567",
            "N° de sécurité sociale : 1 85 05 78 006 084 36",
            "Sécu 185057800608436",
            "CNI : 880692310285",
            "Carte d'identité n° X4RTBPFW4",
            "Numéro fiscal : 1234567890123",
            "IBAN FR76 3000 6000 0112 3456 7890 189",
            "RIB : FR7630006000011234567890189",
            "Carte bancaire 4970-1012-3456-7890",
            // en
            "Passport number: 123456789",
            "SSN: 123-45-6789",
            "Social security number 078-05-1120",
            "National Insurance number: QQ 12 34 56 C",
            "NI number AB123456C",
            "Visa card 4111 1111 1111 1111",
            "Driving licence: MORGA753116SM9IJ",
            "IBAN GB29 NWBK 6016 1331 9268 19",
            // de
            "Reisepassnummer: C01X00T47",
            "Personalausweis: L01X00T47",
            "Steuer-ID: 12 345 678 901",
            "Sozialversicherungsnummer: 65 170839 J 003",
            "IBAN: DE89 3704 0044 0532 0130 00",
            "Kreditkarte 5500 0000 0000 0004",
            // es
            "DNI: 12345678Z",
            "Mi dni es 12345678Z",
            "12345678Z",
            "NIE X1234567L",
            "Número de la Seguridad Social: 28 1234567840",
            "Pasaporte PAA123456",
            "IBAN ES91 2100 0418 4502 0005 1332",
        ] {
            assert!(id_number(fact), "{fact}");
        }
    }

    #[test]
    fn contact_forms() {
        assert_eq!(email(" Lea@Example.org ").as_deref(), Some("lea@example.org"));
        assert_eq!(email("léa chez gmail"), None);
        assert_eq!(phone("06 12 34 56 78", "fr").as_deref(), Some("+33612345678"));
        assert_eq!(phone("+33 6 12 34 56 78", "en").as_deref(), Some("+33612345678"));
        assert_eq!(phone("0033612345678", "de").as_deref(), Some("+33612345678"));
        assert_eq!(phone("612 345 678", "es").as_deref(), Some("+34612345678"));
        assert_eq!(phone("C-1", "fr"), None);
        assert_eq!(phone("12", "fr"), None);
    }
}
