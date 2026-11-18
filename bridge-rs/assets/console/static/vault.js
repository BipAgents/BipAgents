// The vault's news on the console's home (vault keys, lot 2; bridge-rs/src/vault.rs): the lines told once
// (`notices`), the entries an agent could not take (`refused`: agent id -> a code; the sentence is written here, in the
// browser's language) and a second password kept for a login (`duplicate_of`). Pure: tested in node
// (bridge-rs/tests_kit/console_vault.test.mjs).

const plain = text => (text || "").trim().normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

function hostOf(origin) {
  try { return origin ? new URL(origin).host : ""; } catch { return ""; }
}

/** `listing` is `GET /v1/vault`'s body, `T` the console's words. Only what needs the person. */
export function vaultNews(listing, T) {
  const items = listing?.items || [];
  const byId = Object.fromEntries(items.map(i => [i.id, i]));
  const names = Object.fromEntries((listing?.agents || []).map(a => [a.id, a.name || a.id]));
  const notices = (listing?.notices || []).filter(n => n?.id && n.text?.trim());
  const entries = items.map(item => {
    // one code today (its agent could not take it); a newer one reads the same
    const lines = Object.keys(item.refused || {}).sort().map(a => ({ text: T.cannotUseYet(names[a] || a), refused: true }));
    const first = item.duplicate_of && byId[item.duplicate_of];
    if (first) {
      const same = !first.label?.trim() || plain(first.label) === plain(item.label);
      lines.push({ text: same ? T.triedFirstSame : T.triedFirst(first.label.trim()), refused: false });
    }
    return { id: item.id, label: item.label, site: item.domain || hostOf(item.origin), lines };
  }).filter(e => e.lines.length);
  return { notices, entries };
}
