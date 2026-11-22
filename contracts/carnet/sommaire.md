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
   (`notebook_write.category`, short, optional); the nightly model only for what stays in « Autres » (S2, §2b).

## 1. Categories

Fixed ids (English, never shown): personal `people, dates, money, home, papers, trips, work, other, health`; family
`shopping, people, dates, home, papers, holidays, school, menus, other`. The apps show `title` from the bridge's
answers (S4 serves them in the app's language); `sommaire.json` `titles_proposed` holds the proposed names in fr,
en, de, es, nl for Alex to confirm (visible text).

- **Where they live.** A page: `categories` (1 or 2) and `categories_by` (`auto` the bridge's words, `agent` an
  agent's `category` or the nightly model's for a page no word placed, `person` the person's correction) in its
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
  nearest to when it was noted; « 1/2 » is not a date, « 14/11 » is. Pages themselves are not in `dates`. **The
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
  run's `categorized_by_model`, its tokens and cost in the run). It reads a page's title, aliases, « En bref » and lines
  (400 characters, no attribution, nothing that touches health; never a page that looks like health, never the private
  part) and answers 1–2 of `agent_may_give`: written `categories_by: agent` (later tables leave it; the person's
  correction still wins). A page it was shown is not sent again while its text stays the same (`categories_tried`:
  fingerprints only).
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
| S1 | `POST /v1/notebook/categorize/undo` | `{undo}` → `Undone`; 404 `undo_gone` once undone, after a newer correction of the same target, or after 20 newer corrections (today's stay, 100 at most) |
| S2 | `GET /v1/notebook/categorize/today` | today's re-filings, each still undoable (`RefiledToday`) |
| S2 | `GET /v1/notebook` `sorted` | the card « Ton Carnet est rangé par catégories » (`SortedCard`), or null |
| S2 | `POST /v1/notebook/sorted` | `{action: ok\|see}` → `{card: null}`; the card is not shown again |
| S1 | MCP `notebook_write.category` | 1–2 of `agent_may_give`, only when the agent is sure |
| S3 | `GET /v1/notebook/index?since=&epoch=&lang=` | the phone's search copy (`Index`): ETag `"<epoch>-v<version>-<languages of its tables>"` (`"e1a2b3c4-v42-fr.en"`: opaque to the phones), 304 when unchanged (`If-None-Match`, the answer not even made), gzip when accepted, 400 `since_invalid`; `since`/`epoch` = what the phone has → only `items` changed and `removed` ids (`full: false`), the tables whole; another epoch, a `since` above the version or unknown → everything (`full: true`). Never Santé, never ID numbers or references, mails' archives or online files' content; 20 000 lines / 8 MB of JSON as sent at most, default keys left out (`truncated`; out first: archived lines oldest first, archived pages, then the other lines oldest first). Dates and feasts as rules: the copy never changes with the day (`sommaire.json` `dates`, `feasts`, `index`) |
| S3 | `GET /v1/notebook/search?q=&lang=&category=&space=&online=&groups=&sommaire=` | `groups=1`: today's answer plus `groups` (by category, in the order of their best result; `count`, the two best `top`), `counts` (`all` and per category: the chips), `online` (`online=1` only: the connected drives the person allowed, online only; else null); `sommaire=1`: only `groups`, `counts`, `online` (today's keys not made); neither: today's answer only (an installed 1.0 app's search never makes the copy); `category`, `space` narrow it; `private=1`: Santé's, behind the lock, read and kept nowhere |
| S3 | `PUT /v1/drives/{service}/carnet {carnet}` | « Chercher aussi ici depuis le Carnet » (Alex's decision 4: off by default, a switch per connected drive): the drive as listed, `carnet` added (`GET /v1/drives` lists it on each drive) |
| S4 | `GET /v1/notebook/summary?space=` | the sommaire ready to show (`Summary`): selector, kept searches with their count, total, categories with count and grey hint, the moment's card, Santé locked |
| S4 | `GET /v1/notebook/category/{id}?space=&filter=` | a category's page (`CategoryPage`): sub-filters with counts, « Cherché ici » (3), the most recent; `category/health?private=1`: Santé's, behind the lock, never in the index |
| S4 | `GET/POST/DELETE /v1/notebook/kept` | kept searches (6 a space), their count from the bridge |
| S4 | `POST /v1/notebook/searched` | a search made (3 a category for « Cherché ici », never shown to the agents) → 204 |
| S5 | `PATCH /v1/notebook/settings {separate_spaces}` | « Séparer Perso, Pro et Famille » (off by default); `?space=all|perso|pro` on summary/category/search/index; `POST /v1/notebook/space {item | path, space}` « Ranger dans… Perso / Pro » (+ `/space/undo`) |
| S6 | `/v1/family-agent/notebook/{summary,index,kept,categorize,category/{id}}` | the family Carnet's, relayed by the members' bridges as today (`FamilySummary`, `FamilyIndex`, `FamilyCategorized`, `FamilyCategoryPage`) |

**Counts.** A category's `count` counts its items (pages, lines, tasks, records, documents); `total` all of them. A
note with two categories counts in each of them, `total` and `counts.all` once (change 5 of 22/11; §1).

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
The bridge's own search (`/search`'s `groups` and `counts`) is the same rule on the same copy.

**The copy's tables** (S3, `notebook/search_words.rs`): about 120 groups of neighbours a language (fr, en, de, es,
nl; each word of a group has the others), date phrases (about 35 a language), the feasts' rules and Easter for this
year and the nine after, and the stop words (the lexicon's, Dutch's and those of a spoken question). `?lang=` on
`/index` adds the app's language's tables to the Carnet's. The fake bridge keeps its draft tables: the parity file's
cases are written on them.

## 4. What the 1.0 apps see

Exactly today's answers, the keys of `additive` added. Concretely, for an app of today: pages, results, tasks,
records and documents gain `categories` (a list of ids) and sometimes `categories_by`/`space`; the page's
`frontmatter` gains the same keys; the home gains `sommaire`, the list of what this bridge's Carnet can do (`sommaire.json` `Capability`: S1
`categories`, `categorize`; S2 `sorted` (the home's `sorted` card and its answer, today's re-filings, the toast);
S3 `index` (the phone's search copy); then `summary`, `category`, `kept`, `searched`, `spaces`, `family` as they
come): a new app uses a route only when it is listed, else the home of today. An old bridge has no `sommaire`.

## 5. Fake bridge (for the iPhone and Android agents)

`fake-bridge/<lang>/` holds one answer per route for the demo Carnet (`App/Resources/Demo/notebook-demo.json`,
`notebook-demo-trip.json`, `family-notebook-demo.json`), dated from 2026-11-22: `categorize.json`,
`categorize-undo.json` (S1, the bridge's own answers), `categorize-today.json` and `home-sorted.json` (S2, the
bridge's own: the pass run over the demo, then its card), `index.json`, `index-since.json`, `search-aniv.json`,
`summary.json`, `summary-pro.json`, `category-papers.json`, `kept.json`, `family-summary.json`, `family-index.json`.
The categories in them are the bridge's (S1's code on the demo); the rest (counts, hints, groups, the index) is
assembled from them by the generator in the shape of this contract, until S3/S4 answer for real: an app built on
them only changes its base URL. Regenerated by `BIPAGENTS_WRITE_FAKE_BRIDGE=1 cargo test --lib write_fake_bridge`.
