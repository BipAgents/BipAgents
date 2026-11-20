// The console's vault news (bridge-rs/assets/console/static/vault.js) on `GET /v1/vault`'s shapes. Run from the repo
// root: node --test bridge-rs/tests_kit/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { vaultNews } from "../assets/console/static/vault.js";

const T = {
  cannotUseYet: name => `${name} ne peut pas encore s’en servir.`,
  triedFirst: label => `Autre mot de passe : « ${label} » est essayé en premier.`,
  triedFirstSame: "Un autre mot de passe pour ce site est essayé en premier.",
};
const listing = {
  agents: [{ id: "vie", name: "Vie" }, { id: "coach", name: "Coach" }],
  notices: [{ id: "notice_1", text: "Ta carte « •••• 4242 » n'a pas été reprise : ajoute-la dans le Coffre." },
            { id: "notice_2", text: "  " }, { text: "no id" }],
  items: [
    { id: "a", label: "Impots", origin: "https://impots.gouv.fr", domain: "impots.gouv.fr", refused: { coach: "agent_refused" } },
    { id: "b", label: "impôts", origin: "https://impots.gouv.fr", duplicate_of: "a" },
    { id: "c", label: "Ameli", origin: "https://ameli.fr" },
    { id: "d", label: "Ameli (Coach)", origin: "https://ameli.fr", duplicate_of: "c" },
    { id: "e", label: "SNCF", origin: "https://sncf-connect.com", duplicate_of: "gone" },
  ],
};

test("notices with an id and a text, in the bridge's order", () => {
  assert.deepEqual(vaultNews(listing, T).notices.map(n => n.id), ["notice_1"]);
});

test("a refusal is a sentence in the console's words, naming the agent, never Hermes", () => {
  const a = vaultNews(listing, T).entries.find(e => e.id === "a");
  assert.deepEqual(a.lines, [{ text: "Coach ne peut pas encore s’en servir.", refused: true }]);
  assert.equal(a.site, "impots.gouv.fr");
  assert.ok(!JSON.stringify(vaultNews(listing, T)).includes("Hermes"));
});

test("a second password quotes the other entry only when its name differs", () => {
  const news = vaultNews(listing, T).entries;
  assert.equal(news.find(e => e.id === "b").lines[0].text, T.triedFirstSame);
  assert.equal(news.find(e => e.id === "b").site, "impots.gouv.fr");
  assert.equal(news.find(e => e.id === "d").lines[0].text, "Autre mot de passe : « Ameli » est essayé en premier.");
});

test("only marked entries; a second password whose other entry is gone has no line", () => {
  assert.deepEqual(vaultNews(listing, T).entries.map(e => e.id), ["a", "b", "d"]);
  assert.deepEqual(vaultNews({ items: [] }, T), { notices: [], entries: [] });
  assert.deepEqual(vaultNews(null, T), { notices: [], entries: [] });
});
