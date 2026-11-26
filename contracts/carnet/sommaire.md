# The Carnet « Le sommaire »: the contract between the bridge and the phones

Plan: `briefs/plan-carnet-sommaire.md` (piste 9, chosen by Alex on 21/11, his decisions 1–6). The machine-readable
half is `sommaire.json` (types, routes, the keys added to today's routes); example answers from the demo Carnet are in
`fake-bridge/` (French and English, made by the bridge's own code: see « Fake bridge » below). One file for the
bridge, the iPhone and Android: what is written here is what each side tests against.

## 0. Rules that never bend

1. **Additive only.** The routes the installed 1.0 apps read (`GET /v1/notebook`, `/pages`, `/search`, `/page`,
   `/records`, `/tasks`, `/documents`, `/settings`, `/offline`) keep every key and every value they have today. The
   sommaire only adds the keys listed in `sommaire.json` `additive` (iOS `Decodable` and Android `ignoreUnknownKeys`
   skip them). No new kind of section, no new `kind` value in today's lists. Checked by
   `bridge/tests_blackbox/test_carnet_sommaire.py::test_the_1_0_apps_see_exactly_todays_answers`: a demo-like Carnet,
   every such route compared as JSON with the 1.0 bridge's answers (rc-1122 78f4553d6, `fixtures/carnet_1_0/`).
2. **Nothing moves.** Categories are a label on the pages (their frontmatter), never a folder: no file changes
   place. A page's text gains `categories:` and `categories_by:` (and `space: pro` with S5) in its frontmatter,
   readable in Obsidian; the 1.0 apps do not read it.
3. **No HTTP cache.** Every Carnet answer carries `Cache-Control: no-store` (today's routes too: the 1.0 apps'
   URLCache keeps no readable copy).
4. **Santé stays locked.** `health` is the private part (`Part::Health`, Domaines › Santé there): its lines are never
   in the phone's copy (`index`), never in a summary count (the line shows `count: null`, `locked: true`), reached
   only through `?private=1` behind the app's lock. Lines of the shared Carnet that look like health (« allergique
   aux noix », « dentiste mardi », `lexicon::is_health`) stay in their own category and in the copy (Alex's decision
   6, 21/11).
5. **The person wins.** `categories_by: person` is never changed by the bridge, an agent or the nightly pass.
6. **No model, no tokens at write time.** The bridge files by words (§2). The agents may say a category
   (`notebook_write.category`, short, optional); the nightly model only for what no word placed and, once after a new
   version of the table, for what the words placed narrowly (S2, §2b).

## 1. Categories

Fixed ids (English, never shown): personal `people, dates, money, home, papers, trips, work, other, health`; family
`shopping, people, dates, home, papers, holidays, school, menus, other`. The apps show `title` from the bridge's
answers (S4 serves them in the app's language); `sommaire.json` `titles_proposed` holds the proposed names in fr,
en, de, es, nl for Alex to confirm (visible text).

- **Where they live.** A page: `categories` (1 or 2) and `categories_by` (`auto` the bridge's words, `agent` an
  agent's `category` or the nightly model's for a page no word placed or placed narrowly, `person` the person's
  correction) in its
  frontmatter. A page without them (written before the
  sommaire, or by hand in Obsidian) is read with the bridge's categories computed on the fly (`auto`), stored at its
  next write (and by S2's one pass over the whole Carnet). A record, a task, a document and a single line have no
  frontmatter: theirs are computed, the person's correction kept in `notebook/state.json` (`category_marks`).
- **What inherits.** A line (point) has its page's categories; a task its page's (its first `[[link]]`), none:
  `other` (the two reminders the bridge makes for a paper's expiry: `papers`); a record its type's (purchase `money` + `papers` when it has a warranty, subscription `money`, contract and
  paper `papers`, booking `trips`); a document its page's (the page it is linked to or named on), else its name's
  words, else `papers` (Alex's decision 4: what the agents make is found, « Papiers » by default).
- **Dates is computed, never stored or given.** An item is also in `dates` when it carries a date from 90 days ago to
  two years ahead, or a yearly one: a line with a date in its text, a task's due date, a record's next date (warranty
  end, renewal, end, expiry, booking day — never a purchase's day). Yearly only when the text says so (a birthday, an
  anniversary, a wedding); a day without a year that says nothing yearly (« devis reçu le 3 octobre ») is the day
  nearest to when it was noted; « 1/2 » is not a date, « 14/11 » is; nor, without a word saying a day (« le ») or a year, a mark (« 8/10 », « 15/20 »), a size by age (« 10/12 ans ») or « 24/7 » (26/11: a line of tastes was in Dates). Pages themselves are not in `dates`. **The
  index never depends on the day** (so its version and ETag change only with the Carnet): no `dates` among its
  categories; each item carries `date` (a day) or `yearly` (« MM-DD »), and the phone adds `dates` (`yearly` always,
  a `date` within `dated`: 90 days back, 730 ahead). The bridge's live answers (search, summary, a category's page,
  today's routes) give `dates` as of the bridge's day.
- **Autres** (`other`): what no word places. Shown only when not empty.
- **At most two**, the first the main one (the one a result is grouped under). A note with two categories counts in
  each (a category's `count`, a search's `counts.<id>`); `total` and `counts.all` count each note once; a search
  lists a note once, under its first category.

## 2. How the bridge files (S1)

At each write of a page (`notebook_write`, the app's new page), unless the page has `categories_by: person`:

1. the agent's `category` when given (valid ids of `agent_may_give`) → `categories_by: agent`;
2. a page that already has categories keeps them (an `auto` page in `other` is filed again);
3. else the rules learned from the person's corrections (most recent first), then the table → `auto`.

The table (`bridge-rs/src/notebook/categories.rs`, the bridge is the reference; the words of HermesKit's
`NotebookIcon` table copied and extended, fr/en/de/es/**nl**): « Toi » and person pages `people` (a colleague, or a
page a Pro agent wrote: `work` + `people`); the group pages Amis, Famille élargie, Connaissances `people`, Collègues
`work` + `people`; trips `trips`; records pages by type; subjects and areas by their words — the title counts most,
then the aliases, the « En bref », the sections' titles (« Chiffres et budget » → money) and labels, amounts. What
decided is kept for learning (the word: « cuisine » → `home`).

**A correction teaches.** `POST /v1/notebook/categorize` (by an Item's `id`, or by path, record, document, task or
line) on a page sets `categories_by: person`; when the bridge itself had filed it (`auto`) by a word, it keeps the rule « word → the person's categories » for this Carnet (`state.json`
`category_rules`, 200 at most, the most recent wins), used by the next writes and the nightly pass. A correction on a
record, a task, a line or a document decided by its page or its type teaches nothing (too broad); one on a document
decided by a word of its name does; correcting an agent's or an earlier correction teaches nothing. Undo
(`/categorize/undo`) puts back the categories and forgets the rule; one undo a target (the newest). The state keeps
keys only, never a line's text, a page's path or a file's name. `categories` written by hand that the bridge cannot
read (« [[Livres]] », three values) are never touched (409 `categories_by_hand`). A work agent's page is Travail only
when that agent makes it.

**What the app shows** (S2): the answer's `toast` (« Rangé dans Argent. », « … et Maison » for two) and `undo_title`
(« Annuler ») in the app's `?lang=`; then today's re-filings (`GET /v1/notebook/categorize/today`, the newest first),
each with its `undo`: every re-filing of the day stays undoable (the 20 latest, and today's up to 100). A re-filing
whose page, line, task or record is gone (deleted, forgotten) is not listed.

## 2b. The Carnet from before the sommaire (S2)

Once, at night (the nightly pass, before its model; `bridge-rs/src/notebook/categories_pass.rs`), without a model:

- **Every page of the shared part gets `categories` + `categories_by: auto`** in its frontmatter: the categories the
  bridge read on the fly for it until then (§2; one exception: a page a work agent made before S1, read by its words
  until then, is `work`, as S1 files a page a work agent makes). Nothing moves; a page's text
  changes only by those lines, where the bridge writes them (before `updated`, else at the end of the frontmatter; a
  page without one gets one), checked by reading the page back. Left as they are: the tasks page, « À ranger », the
  documents' index (not notes); `categories_by: person` or `agent`; a `categories` the bridge did not write (by hand in
  Obsidian: without `categories_by`, or values it does not read); a text it cannot change safely (another line ending,
  a byte-order mark, YAML it does not parse). Each page written is one change of its history (`categorize`, by the
  « rangement », no summary), never in « Nouveau »: the only thing a 1.0 app shows of it, a line in the page's
  history. A page a work agent made (its last « create » in the history; not one it only wrote on, nor one deleted or
  made again by the person since): `work`. A page with another line ending (CRLF) is left as it is, and so is an
  empty frontmatter followed further down by a rule (its lines would show in the text). A page the bridge cannot
  write is left and tried again the next night. The categories' lines of the pass, of the nightly model and of an
  agent never make the bridge the page's last writer (the nightly pass folds and takes out only pages the person did
  not write last); the person's « Ranger ailleurs » does: the page is theirs. « Ranger ailleurs » and its undo change
  the header lines only.
- **Records, tasks, documents and lines** have no frontmatter: theirs stay computed from their page or type (§1).
- **Once per version**: `state.json` `categories_v`. A better table (version + 1) re-decides the `auto` pages once,
  never `agent` nor `person`. Between versions, each night: the pages without categories and those `auto` in `other`
  (the person's rules may place them now). By batches of 50 pages under the Carnet's lock; idempotent; a pass cut short
  starts again after its last batch (`categories_pass`).
- **What stays in `other`**: the nightly pass's small model, 20 pages a night at most, inside its monthly cap (the
  run's `categorized_by_model`, its tokens and cost in the run): the pages no word placed (never one a hobby's word put
  in `other` on purpose). **Once after each new version** (`categories_once`, carnet-categories): up to 200 pages, those
  no word placed then those the words placed narrowly (never by the person's rules, never a work agent's), by calls of
  20, each call's most cost reserved before it goes (a call that left without an answer is counted at it, its pages
  never sent again), the rest the following nights. It reads a page's title, aliases, « En bref » and lines
  (400 characters, no attribution, nothing that touches health in any of the five languages; never a page whose title
  touches health or that holds a health line, never the private part) and answers 1–2 of `agent_may_give`, or « other »: a real category that
  changes the page is written `categories_by: agent` (later tables leave it; the person's correction still wins);
  anything else leaves the page as the words placed it. A page it was shown is not sent again while its text stays the
  same (`categories_tried`: fingerprints only). One pass at a time (the night's and « ranger maintenant »).
- **The card**, once after the first pass over a Carnet with notes: the home's `sorted` (`SortedCard`; S4 serves it as
  the summary's `card` `{kind: sorted}`): `title` « Ton Carnet est rangé par catégories », `text` « 9 notes dans
  Autres » (null when none: then no `see`), `see` « Voir » opens `open.category` (`other`), `ok` « OK »;
  `POST /v1/notebook/sorted {action: ok|see}` → `{card: null}`, never shown again. Its count is kept until the Carnet
  changes (its history, the person's marks). Visible texts: the plan's in French;
  the other languages proposed, for Alex to confirm (as `titles_proposed`).

## 3. Routes

Types and examples: `sommaire.json` `types`, `routes`, `fake-bridge/`. All under the bridge key, JSON, the app's
`?lang=` for the texts. Errors: the Carnet's of today (`{"detail": {"code", "text"}}`, the HTTP status and the codes
in `routes[].errors`; S1's codes have no text yet: the apps show their own until Alex approves the texts).

| Brick | Route | What |
|---|---|---|
| S1 | `POST /v1/notebook/categorize` | « Ranger ailleurs »: `{item}` (an Item's id) · `{path, private?}` · `{record: {type, id}}` · `{document: {agent, file}}` · `{task}` · `{line: {path, text}}`, with `categories` (1–2 of `person_may_give`) → `Categorized` (`before`, `learnt`, `undo`; S2: `toast`, `undo_title`) |
| S1 | `POST /v1/notebook/categorize/undo` | `{undo}` → `Undone`; 404 `undo_gone` once undone, after a newer correction of the same target, after 20 newer corrections (today's stay, 100 at most), or when the page is gone (the rule it taught and the undo are forgotten all the same) |
| S2 | `GET /v1/notebook/categorize/today` | today's re-filings, each still undoable (`RefiledToday`) |
| S2 | `GET /v1/notebook` `sorted` | the card « Ton Carnet est rangé par catégories » (`SortedCard`), or null |
| S2 | `POST /v1/notebook/sorted` | `{action: ok\|see}` → `{card: null}`; the card is not shown again |
| S1 | MCP `notebook_write.category` | 1–2 of `agent_may_give`, only when the agent is sure |
| S3 | `GET /v1/notebook/index?since=&epoch=&lang=` | the phone's search copy (`Index`): ETag `"<epoch>-v<version>-<languages of its tables>"` (`"e1a2b3c4-v42-fr.en"`: opaque to the phones), 304 when unchanged (`If-None-Match`, the answer not even made), gzip when accepted, 400 `since_invalid`; `since`/`epoch` = what the phone has → only `items` changed and `removed` ids (`full: false`), the tables whole; another epoch, a `since` above the version or unknown → everything (`full: true`). Never Santé, never ID numbers or references, mails' archives or online files' content; 20 000 lines / 8 MB of JSON as sent at most, default keys left out (`truncated`; out first: archived lines oldest first, archived pages, then the other lines oldest first). Dates and feasts as rules: the copy never changes with the day (`sommaire.json` `dates`, `feasts`, `index`) |
| S3 | `GET /v1/notebook/search?q=&lang=&category=&space=&online=&groups=&sommaire=` | `groups=1`: today's answer plus `groups` (by category, in the order of their best result; `count`, the two best `top`), `counts` (`all` and per category: the chips), `online` (`online=1` only: the connected drives the person allowed, online only; else null); `sommaire=1`: only `groups`, `counts`, `online` (today's keys not made); neither: today's answer only (an installed 1.0 app's search never makes the copy); `category`, `space` narrow it; `private=1`: Santé's, behind the lock, read and kept nowhere |
| S3 | `PUT /v1/drives/{service}/carnet {carnet}` | « Chercher aussi ici depuis le Carnet » (Alex's decision 4: off by default, a switch per connected drive): the drive as listed, `carnet` added (`GET /v1/drives` lists it on each drive) |
| S4 | `GET /v1/notebook/summary?space=&lang=` | the sommaire ready to show (`Summary`, §3b): kept searches with their count, total, categories with count and grey words, « Toi » first in Personnes, the moment's card, Santé locked; `spaces`, the selector (§3c); 400 `space_unknown` |
| S4 | `GET /v1/notebook/category/{id}?space=&filter=&lang=` | a category's page (`CategoryPage`, §3b): its pages ready to show (`pages`, `groups`: Alex 26/11), Papiers' sub-filters with counts, « Cherché ici » (3), the latest notes for the 1.1 apps (20, of `filter` when given); `category/health?private=1`: Santé's, behind the lock, read now and kept nowhere (no sub-filter, no search kept); 404 `category_unknown`, 400 `filter_unknown`, 403 `health_locked` |
| S4 | `GET/POST/DELETE /v1/notebook/kept` | kept searches (6 a space, `state.json`), their count from the bridge; POST `{q, title?, space?, category?}` or `{id, title}`, DELETE `?id=` → the space's `KeptList`; 409 `kept_full`, 404 `kept_unknown`, 400 `kept_empty`, `number_refused`, `category_not_given` |
| S4 | `POST /v1/notebook/searched` | `{q, category?, space?, private?}`: a search made (3 a category for « Cherché ici » and the grey words, `searched.json`, never shown to the agents, never in the copy; with Santé open — `category: health` or `private: true` — never kept) → 204; `DELETE ?q=&space=&category=`: the one of these words, or all → 204 |
| S5 | `GET/PATCH /v1/notebook/settings {separate_spaces}` | « Séparer Perso, Pro et Famille » (off at first; also the home's `settings.separate_spaces`; 422 not a bool) |
| S5 | `POST /v1/notebook/categorize` `space?` | « Ranger dans… Perso / Pro » (§3c): `space` (`perso`, `pro`) alone or with `categories` → `Categorized` + `space`, `before.space`, the toast saying it; one undo (`/categorize/undo` → `Undone` + `space`); today's re-filings say it (`Refiled.space`); 400 `space_unknown` |
| S5 | `?space=all\|perso\|pro` | on summary (`spaces`: the selector), category, kept, searched (S4), search (400 `space_unknown` now; ignored with `private=1`) and index (one space's copy, its own ETag); `POST /v1/notebook/page` `space?` (the app's open tab) |
| S6 | `/v1/family-agent/notebook/{summary,category/{id},search,index,kept,searched,categorize,categorize/undo}` | the family Carnet's (§6), relayed by the members' bridges as today (`FamilySummary`, `FamilyCategoryPage`, `FamilySearch`, `FamilyIndex`, `KeptList`, `FamilyCategorized`, `FamilyUndone`) |

**Counts.** A search's `counts.<id>` counts its items (pages, lines, tasks, records, documents); `counts.all` all of
them. A note with two categories counts in each of them, `counts.all` once (change 5 of 22/11; §1). The sommaire's
category `count` counts the pages its screen lists (§3b « Pages », Alex 26/11); its `total` the notes.

**Items.** Everything the phone lists (index entries, search groups, a category's recent ones) is an `Item`: `id`
(stable, never a text, a path or a file's name: `page:<page key>` — its `id`, else `f` + a hash of its path —,
`line:<page key>:<hash of the line>` with `:2`, `:3` for the same line said again, `task:<id>`, `record:<id>`,
`doc:<agent>:<hash of the file's name>`), `kind`, `title`, `line` (240 characters; never a number or a reference:
`sommaire.json` `records`), `categories`, `space` (`perso` or `pro`), `date` / `yearly`, `source`, `who` and
`noted`, `archived`, `label` / `answer` (a « Label : value » line), `page` (a line's page's id: its title and
aliases apply to the line), `open` (what a tap opens). Index entries add `fold` (the words folded by the bridge) and,
on a page, its `aliases`. `categorize` takes an Item's `id` (`{item}`).

**The phone's search** (A1, D1) follows the rule written once at the bridge (S3: `search-parity.json`, played by the
three: the bridge's `notebook::finder`, HermesKit's and Android's `CarnetSearch`): fold, prefix from 3 letters, one
typo from 5 letters and two from 8 (never towards a word under 4), the neighbours' table (`neighbours`), the date
phrases (`dates`: an interval computed on the phone; feasts as rules, `dates.feasts` a fixed day or days from Easter
with `dates.easter`, never days), stop words dropped unless alone; ranking as in the plan §3.3 and the file's `rule`.
The bridge's own search (`/search`'s `groups` and `counts`) is the same rule on the same copy. The phone never keeps
Santé (an item whose `part` is not `main`, or filed in `health`, is left out when read), whatever a bridge sends.

**The copy's tables** (S3, `notebook/search_words.rs`): about 120 groups of neighbours a language (fr, en, de, es,
nl; each word of a group has the others), date phrases (about 35 a language), the feasts' rules and Easter for this
year and the nine after, and each language's stop words (its words of the lexicon's, and those of a spoken question).
`?lang=` on
`/index` adds the app's language's tables to the Carnet's (stop words included).

**Never in the copy** (S3, review 1 and 2): the private part (Santé), a line holding an ID number or a reference
(`holds_reference`), an archived mail's link; a page whose **path** holds a reference (its name or a folder's: the page
and its lines are left out, the phone opening a page by its path), a document whose **file's name** holds one, an
agent's web page whose title does (a real file's extension is no part of a reference: « Budget 2026.xlsx » stays,
« 12345.AB1 » does not); a reference in a title, an « En bref », a section's title (`source`), a task's title or a
document's name is masked (« … »). A document's text: its lines holding a number or a reference left out on the raw
line, then no word of digits but a year (19xx, 20xx) or a number of one or two digits. Two files of the same name an
agent received: one item, the newest. The fake bridge keeps its draft tables: the parity file's
cases are written on them.

## 3b. The sommaire (S4)

`bridge-rs/src/notebook/summary.rs`; the rules in full: `sommaire.json` `summary`.

- **Counted on the phone's copy** (the index's items), Santé never there. A category's line is shown when its screen
  lists a page; Santé's always (but in `pro`), locked: `count: null`, no word, no `first`.
- **Pages** (Alex 26/11, « Personnes > liste de personnes > Sujets… Maison > liste des sujets > liste des notes »;
  `bridge-rs/src/notebook/structure.rs`): a category's screen lists its **pages** (`CategoryPage.pages`,
  `CategoryEntry`), each with its number of notes; a tap opens the page (its sections, their notes). A page is in a
  category by its own `categories` (§1), never by one of its lines; a records' page (Achats, Abonnements…) by its
  records' (`kind: records`, `notes` its records there); a line, a task or a document is never a row (a file is
  never shown by its raw name: « photo.jpg » in Personnes, 26/11). Personnes: « Toi » first (no group), then the
  people by `group` — `family`, `friends`, `work`, `others`, from the head of their « Qui c'est » (else « En bref »):
  « Ton beau-frère, … » → `link` « Beau-frère » —, then the other pages filed there (`subjects`; a group page « Amis »
  or the family's area under its group). Elsewhere: by name, archived last; Sujets then Personnes when both. Dates:
  the pages with a date to come (a yearly one's next, a day within `dated.after`), the soonest first, `next` (the day,
  what it is: its line's label) on each. `groups`: the headers in order, only with two groups or more (their names:
  `categories.groups_proposed`, for Alex). The copy carries what the phones need to make the same list without a
  network: a page's `page_kind`, a person's `link` and `group`, `close`, `icon` (`IndexItem`). A bridge from before
  26/11 sends no `pages`: the app makes them from its copy (the same rule; the examples:
  `fake-bridge/<lang>/pages.json`, made on `index.json`).
- **Grey words** (`hint`): the category's 3 latest searches made, else the titles of its 3 pages last written in (in
  Dates, the soonest). **`first`**: the first page its screen lists (« Toi » in Personnes). **Latest** (`recent`, the
  1.1 apps'): the newest noted first; in Dates, what is to come first.
- **The moment's card** (`card`): one at a time, the first that waits of `sorted`, `ask`, `follow` (today's, still
  open), `misplaced`, `review`, each as its own route shows it; null in Pro.
- **Sub-filters**: Papiers only, counted in pages: Factures (a page's words), Contrats (the contracts' records page,
  or a page's words), Garanties (the purchases' records page, or a page's words); `filter` narrows `pages` (and
  `recent`). Argent's, Dates' and Personnes' went with the lines (Alex 26/11: they meant nothing for a list of
  pages). Their names are proposed (`titles_proposed`, for Alex).
- **Kept searches**: the words as typed (a date phrase is counted again each day), 6 a space, never a number or a
  reference, never Santé; in the phone's copy (`kept`, every space's), the copy's version moving with them.
- **Searches made**: `searched.json`, 3 a category and a space; without a category, under the best result's; never
  Santé, a number, a single letter; never in a page, the state, the copy, nor anything an agent reads.
- **« Oublie ça »** takes the searches with it: the kept ones (every space; the copy moves to a new version, the phones
  drop them) and those made (« Cherché ici », the grey words) whose words, or a kept one's title, say what is forgotten
  (folded as the search folds, whole words; a person's first name too when their page is forgotten whole).
- **Spaces** (S5, §3c): a note's space is its page's or the person's mark; `perso` is what is not Pro. `spaces` is
  the selector (§3c): the setting off, « Moi · Famille » for a member of a family, else `[]`; on, Perso, Pro and
  Famille. Unknown: 400 `space_unknown`.
- **`version` with `epoch`** (the copy's): a version is compared only with its epoch. On a copy cut (`truncated`), the
  counts are the copy's, archives included.
- **Santé open**: a search made in Santé or with `private=1` is sent with `category: health` or `private: true` (or not
  at all): never kept. **`DELETE /v1/notebook/searched`** drops one (by its words) or all: for a search whose words are
  nowhere in the Carnet, that « Oublie ça » cannot find (the gesture comes with A2/D2).
- **Never health in the moment's card**: no point of Santé in `misplaced` (whatever its words; the 1.0 home's card too),
  no `ask`/`follow` whose question, choices or task touch health.
- **Errors** (`{"detail": {"code", "text"}}`): no text yet but `number_refused`'s (the apps show their own until
  Alex approves them); `not_written` (500) when `state.json` cannot be written.

## 3c. Perso and Pro (S5)

`bridge-rs/src/notebook/spaces.rs`; the choices of 17/11 (the plan §2.3): a setting, off at first; 6 B (Famille is the
household's shared Carnet, it has no space); 7 B (the Pro agents read everything); 8 B (nothing is filed by itself).

- **The setting** « Séparer Perso, Pro et Famille » (Réglages › Carnet): `separate_spaces` in `GET/PATCH
  /v1/notebook/settings` (the bridge's `state.json`), off at first. Off, everything is together, as before. It decides
  two things only: where a Pro agent makes its new pages, and the sommaire's selector. Switched off again, nothing is
  lost: the pages keep their space, the sommaire of `all` shows everything (the kept searches of every space too).
- **A page's space**: `space: pro` in its frontmatter (readable and editable in Obsidian; `Pro` in any case is Pro,
  another value Perso), nothing for Perso. Its lines, its tasks (their first `[[link]]`, as their categories) and the
  documents linked to it or named on it follow it. A record is Perso unless the person files it in Pro. A record, a
  task, a document or a single line filed in another space than its page's keeps a mark in `state.json`
  `space_marks` (keys only, never a text: « Oublie ça »).
- **A Pro agent** (the person's « Pro » group of agents, or the work Bip), the setting on: a page it **makes** — a
  person, a subject, an area, a trip — gets `space: pro` (after `categories_by`). Never « Toi », the tasks page,
  « À ranger », the records' and documents' pages, nor the private part. A page that exists keeps its space; the
  setting off, nothing is written. It reads every space, as every agent of the person (7 B).
- **« Ranger dans… Perso / Pro »**: `POST /v1/notebook/categorize` with `space` (`perso` | `pro`), on the page (`space`
  alone: `categories` may be left out) or in « Ranger ailleurs » (with `categories`): one gesture, one `undo` (the
  categories and the space as they were). A page changes by its header's `space` line only (the history's
  `categorize`, the person's write); the others by a mark. The answer: `Categorized`, plus `space` and `before.space`
  (without `space` in the request: S1's answer exactly); `toast` « Rangé dans Pro. », « Rangé dans Argent et Pro. »;
  `undo_title` « Annuler ». `Undone` gains `space` (the space now). Today's re-filings (`categorize/today`): an item
  whose gesture set a space has `space` and the toast's text; a gesture that set only a space is listed too.
- **Each space answers for itself**: `?space=` on summary, category, kept, searched, search and index; `all` (or none)
  everything, `perso` what is not Pro, `pro` Pro's. **The selector** (`Summary.spaces`, S5 and S6, summary.rs
  `selector`): the setting off, « Moi · Famille » (`all` « Moi », then `family` « Famille ») for a member of a family
  (§6), else `[]`; the setting on, `perso` (« Perso »), then `pro` (« Pro ») when the person has a Pro agent or a Pro
  note, then `family` (« Famille ») for a member of a family. `selected`: the space asked (`all` with the setting on
  selects none); `family` is never selected nor asked of these routes: it opens the family Carnet's sommaire (§6). The
  app shows the selector with two choices or more, asks `perso` or `pro` with the setting on (`all` off), and shows one
  Famille: this entry, or its own by the household's rule, never both. **The phone's copy**: every item carries its `space` (`perso` left
  out); `?space=perso|pro` gives that space's notes only, with its own ETag (`"…-pro"`) and its own epoch
  (`<epoch>-pro`: a `since` of another copy, the whole one or the other space's, gets everything), a note that left
  the space in `removed`, the kept searches of that view.
- **Kept searches**: no rewrite when the setting changes; by view: `all` lists every space's, `perso` Perso's and those
  kept in `all` (kept with the setting off: they show in Perso, the tab the app opens on), `pro` Pro's; **six a view**:
  a view shows the six newest (after a change of the setting it may hold more: none is dropped, none more is kept
  until there are fewer). A rename (`POST /kept {id, title, space?}`) or a deletion (`DELETE /kept?id=&space=`) answers
  the list of the view on screen (`space`; none: `all`, the view of the S4 apps). **Searches made**: each space
  its own (« Cherché ici » and the grey words of Pro are Pro's; those of `all` are back when the setting is off).
- **Nothing changes space without the person**: two pages of different spaces are never merged (the nightly pass, a
  contact found on another page, the trips made again: the write goes on the page named); the trips split from a page
  (« Séparer en voyages ») are in its space; a trip of the same name in the other space is another page.
- **« Toi »** is always Perso: « Ranger dans… Pro » on it → 400 `space_not_given` (17/11).
- **Santé is in no space**: never `space` on its pages (one written by hand is not read), never in Pro (no Santé line
  in the Pro sommaire), its page (`category/health?private=1`) and its search (`private=1`) ignore `space`;
  « Ranger dans… » never reaches the private part (`private: true` → 400 `category_not_given`).
- **New page** (`POST /v1/notebook/page`): `space?` (`perso` | `pro`), the app's open tab (« Écrire une page » in Pro).
- **Visible texts** (proposed, for Alex): the setting's name and line, the spaces' names (the home's groups of agents:
  Perso / Pro; en Personal / Work, de Privat / Arbeit, es Personal / Trabajo, nl Privé / Werk), the toast.
- **Errors**: `space_unknown`, `space_not_given` (400), no text yet (as S1's).
- **One gesture, one write**: « Ranger ailleurs » with a space changes a page's text once (its categories and its
  `space` line together, under the Carnet's lock), and so does its undo.

## 4. What the 1.0 apps see

Exactly today's answers, the keys of `additive` added. Concretely, for an app of today: pages, results, tasks,
records and documents gain `categories` (a list of ids) and sometimes `categories_by`/`space`; the page's
`frontmatter` gains the same keys; the home gains `sommaire`, the list of what this bridge's Carnet can do (`sommaire.json` `Capability`: S1
`categories`, `categorize`; S2 `sorted` (the home's `sorted` card and its answer, today's re-filings, the toast);
S3 `index` (the phone's search copy); S4 `summary`, `category`, `kept`, `searched` (the sommaire ready to show);
S5 `spaces` (« Séparer Perso, Pro et Famille », `space` on « Ranger ailleurs »); S6 `family` (the family Carnet's
sommaire, §6)): a new app uses a route only when it is listed, else the home of today. An old bridge has no `sommaire`.
Pages also gain `space` (`perso` or `pro`; null in the private part) on `GET /v1/notebook/page` and `/pages`, and the
settings `separate_spaces`; with the setting on and a Pro agent writing, a 1.0 app still sees every page, as before
(checked: `test_carnet_spaces.py::test_s5_the_1_0_apps_see_the_same_with_the_setting_on_and_a_pro_agent`).

## 5. Fake bridge (for the iPhone and Android agents)

`fake-bridge/<lang>/` holds one answer per route for the demo Carnet (`App/Resources/Demo/notebook-demo.json`,
`notebook-demo-trip.json`, `family-notebook-demo.json`), dated from 2026-11-22: `categorize.json`,
`categorize-undo.json` (S1, the bridge's own answers), `categorize-today.json` and `home-sorted.json` (S2, the
bridge's own: the pass run over the demo, then its card), `index.json`, `index-since.json`, `search-aniv.json`,
`summary.json`, `summary-pro.json`, `category-papers.json`, `kept.json`, `family-summary.json`, `family-index.json`.
The categories in them are the bridge's (S1's code on the demo); the rest (counts, hints, groups, the index) is
assembled from them by the generator in the shape of this contract, until S3/S4 answer for real: an app built on
them only changes its base URL. Regenerated by `BIPAGENTS_WRITE_FAKE_BRIDGE=1 cargo test --lib write_fake_bridge`.

## 6. The family Carnet (S6)

`bridge-rs/src/family_carnet.rs`; the rules in full: `sommaire.json` `family`; the routes: `sommaire.json` `routes`
(brick S6; §3's S6 row in full). The family Carnet is `family-notebook.json` on the place that holds the family's
agents (family_agent.rs); a member's bridge relays her app's calls there as it does today's family routes
(`/family/fa/notebook/…` between the bridges, the household key). Capability `family` in the home's `sommaire`: a new
app shows the family sommaire only when it is listed, and falls back to today's family Carnet on a 404
`family_sommaire_unavailable` (the holder's bridge is from before S6).

| Route (`/v1/family-agent/notebook/…`) | What |
|---|---|
| `GET summary?lang=` | `FamilySummary`: the family's categories in `categories.order.family` (one without a note left out, never Santé), each with its count, grey words (the person's 3 latest searches there, else the 3 latest notes: an item's line, a page's title) and `first`; the household's kept searches with their count; « Avec Marie, Léa et Tom » (`with`: the household's others); `version` and `epoch` of the copy |
| `GET category/{id}?lang=` | `FamilyCategoryPage`: its pages and lists (`pages`, `FamilyCategoryEntry`, Alex 26/11: the person's structure; a list of things by its title, a list of tasks or of dates under each category its items have, `notes` its items there not done; Dates: those with a day to come, the soonest first) and their count, the person's « Cherché ici » (`searched`), the 20 latest notes (Dates: the soonest to come first); 404 `category_unknown` for an id that is not the family's (`money`, `trips`, `work`, `health`) |
| `GET search?q=&category=&lang=` | `FamilySearch`: by category in the order of their best result, the two best each, the chips' `counts` (the rule of `search-parity.json`) |
| `GET index?since=&epoch=&lang=` | `FamilyIndex`: the phone's copy, as `/v1/notebook/index` (ETag `"<epoch>-v<version>-<languages>"`, 304, gzip, `since`/`epoch` diffs); `left: true` (epoch `left`, version 0, nothing in it, ETag `"left"`): this person has no family Carnet (any more) — the phone **erases its copy, its file too**. 502 (the holder does not answer): the phone keeps its copy |
| `GET/POST/DELETE kept` | the household's kept searches, **common** (6, `family-notebook.json` `kept`): POST `{q, title?, category?}` or `{id, title}`, DELETE `?id=` → `KeptList` (`space` null); S4's codes |
| `POST/DELETE searched` | a search made → 204: kept by **the person's own bridge** (`family-searched.json`), never on the holder's place, never told to another member nor to an agent; 3 a category; **the app sends its best group's category**: without one, a member's bridge keeps nothing and sends nothing anywhere (the holder's own: under the best result's); out of the family: 404 `no_family`, nothing kept; DELETE `?q=&category=` |
| `POST categorize`, `POST categorize/undo` | « Ranger ailleurs »: `{page}`, `{list}`, `{list, item}` or `{item}` (a `FamilyIndexItem` id), `categories` 1–2 of `family_may_give` → `FamilyCategorized` (the toast in the app's language), `{undo}` → `FamilyUndone`; S1's codes |

- **Filing** (no model, no token, at each write: the apps', a member's, the family agent's — its tools unchanged): the
  person's rules (her corrections' words) first, then the family's own words (Courses, École, Menus, Vacances, in five
  languages), then the person's Carnet's table with the family's ids (Argent → Papiers, Voyages → Vacances, Travail →
  Autres). A list of things (« Courses ») is filed by its title and its items follow it; an item of a list of tasks
  has its own words' (none: Autres); an item of a list of dates its own words', or none: it is then in Dates only (a
  birthday is `yearly`; the search groups such a note under Dates when dated, else Autres: the phones' family search
  does the same, finder.rs; parity cases `family-no-category-*`, `parity-copies/family-edges.json`). Pages by their title, then their text. A renamed page or list the bridge had filed is filed
  again; **the person's choice is never changed**. The family Carnet gets **no nightly model pass** (small, every
  member's writing, the holder's model is one person's: status-carnet-s6.md).
- **Ids**: `fam:list:<list>/<item>`, `fam:page:<page>` (`open.path` is the page's id). Never in the copy: a line holding
  a number or a reference (the person's copy's rule).
- **« Passage de main »** (the holder unlinks a member: today's handover, « Le reprendre »): the handover carries the
  pages and lists with their categories, the kept searches and the rules; taken over, the Carnet has another epoch and
  the phones take it whole. The searches made never move: each person's are on her own place.
- **Leaving** (taken out of the family agent by its creator, a link removed on either side, the holder gone): `left:
  true` from the index, 404 `no_family` from the other routes; the person's searches made are erased (unless a
  handover waits for her to take the Carnet over). The holder's 401 `unknown household` (it removed the link, its
  handover never came) is leaving too; another 401 or 403 of the holder's is a 502, never passed to the app. Her kept
  searches leave the holder's place: the holder drops those of a household in none of its family agents (taken out,
  link removed there), her bridge drops hers as she removes the link (before the link goes, so through Kerfy Studio's
  relay too), and a handover gives the receiver hers (`~self`) and leaves the leaving holder's behind. Who kept a
  search is kept by household (`by_household`: a household's id, or `~self` for the place's own person — never a
  household's id, a slug of its name), never a name. Temporary files a hard stop left (`family-notebook.<hex>.tmp`,
  `family-agent.<hex>.tmp`, older than a minute) go as the bridge starts.
- **« Oublie ça »**: on the holder's place, the person's own lines (as before), her own kept searches and her searches
  made that say it (the others' are theirs), written under the family Carnet's own lock; on a member's place, her own
  searches made, and her own kept searches held by the holder: her bridge asks which ones her household kept
  (`GET /family/fa/notebook/kept?mine=1`, between the bridges), matches them itself and deletes those by id — the
  forgotten words never leave her place; `POST /v1/notebook/forget/confirm` adds `family: "not_reached"` when the
  holder did not answer (nothing kept to try again). Her lines on the holder's place are not reached (as before S6;
  Alex, 23/11: the forgotten words are not relayed to the holder).
- **« Ranger ailleurs »** on a list of tasks or of dates: 400 `list_not_filed` (it has no category of its own).
- **The person's own sommaire**: its selector (`Summary.spaces`, §3c) has `family` (« Famille ») for a member of a
  family (a holder whose family agent has at least one other household, or a member of a linked household's family
  agent): after « Moi » (`all`) with the setting « Séparer Perso, Pro et Famille » off, after Perso and Pro with it on.
- **The 1.0 apps**: `GET /v1/family-agent/notebook`, `POST /v1/family-agent/notebook` and the family agent's tools
  answer exactly as before (the categories kept in the file are left out of them; checked against the 1.0 bridge's
  answers: `test_family_carnet_sommaire.py::test_s6_the_1_0_apps_see_todays_family_carnet`).
