// The Bips, drawn as the apps and the site draw them (the pixel Bips): pixel.js is the site's engine
// (site/src/shared/bips.js) and pixel-data.js its numbers (scripts/pixel/pixel.json), both copied here by
// scripts/pixel/gen.py (scripts/pixel/test_console_pixel.py checks they are the same). Until 27/11 this file drew the
// Bips from before the pixel ones. Only constants and numbers go into the SVG: no text from an agent or a server ever
// reaches this markup.
import { pixelSVG, REDUCED, CATEGORIES as PIXEL } from "./pixel.js";

const WORDS = {
  wellness: ["Bien-être", "Wellness"], finance: ["Finance", "Budget"], daily: ["Quotidien", "Vie"], work: ["Travail", "Boulot"],
  learning: ["Apprendre", "Prof"], home: ["Maison", "Maison"], creative: ["Créatif", "Atelier"], tech: ["Tech", "Serveur"],
};
// each kind's colours as in the apps (main, deep: the Bip's body; tint: the site's pale card ground)
export const CATEGORIES = Object.values(PIXEL).map(c => ({
  id: c.id, label: WORDS[c.id][0], name: WORDS[c.id][1], main: c.main, deep: c.deep, tint: c.tint, pitch: c.pitch,
}));
// the console's moods → the pixel Bips' states (pixel.js knows idle, thinking, speaking, content…)
const MOODS = { asking: "listening" };

export function renderBip(c, s, t, size = 48) {
  return pixelSVG({ category: c.id, mood: MOODS[s.mood] || s.mood, t, size, still: REDUCED });
}

// ----------------------------------------------------------------------------------------------------------------
// English by default, French with ?lang=fr or the FR button (remembered), or a French browser.

// The app's guess from a name (AgentCategory.suggest): the category the app picked is not on the server yet.
const KEYWORDS = {
  wellness: ["well", "santé", "sante", "health", "sommeil", "sleep", "sport", "fitness", "bien-être", "bien etre", "médit", "nutrition", "coach", "salud", "bienestar", "sueño", "gesund", "schlaf"],
  finance: ["financ", "budget", "banque", "bank", "argent", "money", "invest", "compta", "impôt", "impot", "tax", "dinero", "banco", "impuesto", "geld", "finanz", "steuer"],
  daily: ["vie", "life", "perso", "agenda", "quotidien", "organis", "assistant", "planning", "famille", "family", "daily", "vida", "familia", "alltag", "leben", "familie"],
  work: ["travail", "work", "pro", "bureau", "client", "projet", "business", "mail", "job", "trabajo", "oficina", "arbeit", "büro"],
  learning: ["appren", "learn", "étude", "etude", "langue", "cours", "school", "prof", "tutor", "aprend", "estudi", "idioma", "escuela", "lern", "schule", "sprach"],
  home: ["maison", "home", "domot", "jardin", "cuisine", "courses", "bricol", "garden", "kitchen", "casa", "hogar", "cocina", "haus", "garten", "küche"],
  creative: ["créa", "crea", "écri", "ecri", "write", "musique", "music", "design", "photo", "art", "escrit", "música", "creativ", "schreib", "musik", "kunst", "kreativ"],
  tech: ["tech", "dev", "code", "serveur", "server", "infra", "ops", "admin", "linux", "servidor", "technik"],
};

export function category(agent) {
  const wanted = agent.category && CATEGORIES.find(c => c.id === agent.category);
  if (wanted) return wanted;
  const text = `${agent.name || ""} ${agent.id || ""}`.toLowerCase();
  let best = null, score = 0;
  for (const c of CATEGORIES) {
    const n = KEYWORDS[c.id].filter(k => text.includes(k)).length;
    if (n > score) { best = c; score = n; }
  }
  return best || CATEGORIES.find(c => c.id === "daily");
}

// One animation loop for every Bip on the page, at most 30 frames a second (as the site's); each draws itself into
// its element.
const drawn = new Set();
let started = 0, last = 0;

function frame(now) {
  if (!drawn.size) return;
  requestAnimationFrame(frame);
  if (last && now - last < 32) return;
  last = now;
  const t = (now - started) / 1000;
  for (const bip of drawn) bip.el.innerHTML = renderBip(bip.c, bip.state, REDUCED ? 0.5 : t, bip.el.clientWidth || 48);
}

export function attach(el, c, mood = "idle") {
  const bip = { el, c, state: { mood } };
  const first = drawn.size === 0;
  drawn.add(bip);
  if (first) { started = started || performance.now(); last = 0; requestAnimationFrame(frame); }
  return {
    set(c2, mood2) { bip.c = c2; bip.state.mood = mood2; },
    detach() { drawn.delete(bip); },
  };
}
