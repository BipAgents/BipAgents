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
   (`notebook_write.category`, short, optional); the nightly model only for what stays in « Autres » (S2).

## 1. Categories

Fixed ids (English, never shown): personal `people, dates, money, home, papers, trips, work, other, health`; family
`shopping, people, dates, home, papers, holidays, school, menus, other`. The apps show `title` from the bridge's
answers (S4 serves them in the app's language); `sommaire.json` `titles_proposed` holds the proposed names in fr,
en, de, es, nl for Alex to confirm (visible text).

- **Where they live.** A page: `categories` (1 or 2) and `categories_by` (`auto` the bridge's words, `agent` an
  agent's `category`, `person` the person's correction) in its frontmatter. A page without them (written before the
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

## 3. Routes

Types and examples: `sommaire.json` `types`, `routes`, `fake-bridge/`. All under the bridge key, JSON, the app's
`?lang=` for the texts. Errors: the Carnet's of today (`{"detail": {"code", "text"}}`, the HTTP status and the codes
in `routes[].errors`; S1's codes have no text yet: the apps show their own until Alex approves the texts).

| Brick | Route | What |
|---|---|---|
| S1 | `POST /v1/notebook/categorize` | « Ranger ailleurs »: `{item}` (an Item's id) · `{path, private?}` · `{record: {type, id}}` · `{document: {agent, file}}` · `{task}` · `{line: {path, text}}`, with `categories` (1–2 of `person_may_give`) → `Categorized` (`before`, `learnt`, `undo`) |
| S1 | `POST /v1/notebook/categorize/undo` | `{undo}` → `Undone`; 404 `undo_gone` after 20 newer corrections |
| S1 | MCP `notebook_write.category` | 1–2 of `agent_may_give`, only when the agent is sure |
| S3 | `GET /v1/notebook/index?since=&epoch=` | the phone's search copy (`Index`): ETag `"<epoch>-v<version>"`, 304 when unchanged; `since`/`epoch` = what the phone has → only `items` changed and `removed` ids (`full: false`), the tables whole; another epoch, a `since` above the version or unknown → everything (`full: true`). Never Santé, never ID numbers or references, mails' archives or online files' content; 20 000 lines / 8 MB of JSON as sent at most, default keys left out (`truncated`; out first: archived lines oldest first, archived pages, then the other lines oldest first). Dates and feasts as rules: the copy never changes with the day (`sommaire.json` `dates`, `feasts`, `index`) |
| S3 | `GET /v1/notebook/search` | today's answer plus `groups` (by category, in the order of their best result; `count`, the two best `top`), `counts` (`all` and per category: the chips), `online` (connected drives the person allowed, online only) |
| S4 | `GET /v1/notebook/summary?space=` | the sommaire ready to show (`Summary`): selector, kept searches with their count, total, categories with count and grey hint, the moment's card, Santé locked |
| S4 | `GET /v1/notebook/category/{id}?space=&filter=` | a category's page (`CategoryPage`): sub-filters with counts, « Cherché ici » (3), the most recent; `category/health?private=1`: Santé's, behind the lock, never in the index |
| S4 | `GET/POST/DELETE /v1/notebook/kept` | kept searches (6 a space), their count from the bridge |
| S4 | `POST /v1/notebook/searched` | a search made (3 a category for « Cherché ici », never shown to the agents) → 204 |
| S5 | `PATCH /v1/notebook/settings {separate_spaces}` | « Séparer Perso, Pro et Famille » (off by default); `?space=all|perso|pro` on summary/category/search/index; `POST /v1/notebook/space {item | path, space}` « Ranger dans… Perso / Pro » (+ `/space/undo`) |
| S6 | `/v1/family-agent/notebook/{summary,index,kept,categorize,category/{id}}` | the family Carnet's, relayed by the members' bridges as today (`FamilySummary`, `FamilyIndex`, `FamilyCategorized`, `FamilyCategoryPage`) |

**Counts.** A category's `count` counts its items (pages, lines, tasks, records, documents); `total` all of them.
The fake bridge counts so; S4 may count otherwise (pages apart), the contract's shape stays.

**Items.** Everything the phone lists (index entries, search groups, a category's recent ones) is an `Item`: `id`
(stable, never a text, a path or a file's name: `page:<page key>` — its `id`, else `f` + a hash of its path —,
`line:<page key>:<hash of the line>` with `:2`, `:3` for the same line said again, `task:<id>`, `record:<id>`,
`doc:<agent>:<hash of the file's name>`), `kind`, `title`, `line` (240 characters; never a number or a reference:
`sommaire.json` `records`), `categories`, `space` (`perso` or `pro`), `date` / `yearly`, `source`, `who` and
`noted`, `archived`, `label` / `answer` (a « Label : value » line), `page` (a line's page's id: its title and
aliases apply to the line), `open` (what a tap opens). Index entries add `fold` (the words folded by the bridge) and,
on a page, its `aliases`. `categorize` takes an Item's `id` (`{item}`).

**The phone's search** (A1, D1) follows the rule written once at the bridge (S3: `search-parity.json`, played by the
three): fold, prefix from 3 letters, one typo from 5 letters and two from 8 (never on names under 4), the
neighbours' table (`neighbours`), the date phrases (`dates`: an interval computed on the phone, moving feasts as
absolute dates from the bridge), stop words dropped unless alone; ranking as in the plan §3.3. The phone never keeps
Santé (an item whose `part` is not `main`, or filed in `health`, is left out when read), whatever a bridge sends.

## 4. What the 1.0 apps see

Exactly today's answers, the keys of `additive` added. Concretely, for an app of today: pages, results, tasks,
records and documents gain `categories` (a list of ids) and sometimes `categories_by`/`space`; the page's
`frontmatter` gains the same keys; the home gains `sommaire`, the list of what this bridge's Carnet can do (`sommaire.json` `Capability`: S1
`categories`, `categorize`; then `index`, `summary`, `category`, `kept`, `searched`, `spaces`, `family` as they
come): a new app uses a route only when it is listed, else the home of today. An old bridge has no `sommaire`.

## 5. Fake bridge (for the iPhone and Android agents)

`fake-bridge/<lang>/` holds one answer per route for the demo Carnet (`App/Resources/Demo/notebook-demo.json`,
`notebook-demo-trip.json`, `family-notebook-demo.json`), dated from 2026-11-22: `categorize.json`,
`categorize-undo.json` (S1, the bridge's own answers), `index.json`, `index-since.json`, `search-aniv.json`,
`summary.json`, `summary-pro.json`, `category-papers.json`, `kept.json`, `family-summary.json`, `family-index.json`.
The categories in them are the bridge's (S1's code on the demo); the rest (counts, hints, groups, the index) is
assembled from them by the generator in the shape of this contract, until S3/S4 answer for real: an app built on
them only changes its base URL. Regenerated by `BIPAGENTS_WRITE_FAKE_BRIDGE=1 cargo test --lib write_fake_bridge`.
