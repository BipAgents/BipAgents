//! « Entre tes agents » as rules, not pairs (Alex 21/11: piste A « Trois phrases », « C'est le plus clair de
//! tous »). Before, « Qui peut écrire à qui » was one switch per directed pair of agents (peers.json) and « Qui peut lire
//! quoi » one per reader and shared folder (shares.json): a family of 6 people with 7 agents each would have had 3,444
//! switches. Rules make it about 21, and a new agent is covered at once, nothing to tick.
//!
//! - **Your agents** (`write`): « Tes agents peuvent s'écrire » none | some | all; under `some`, the explicit pairs
//!   (`pairs`, « liens à part »), kept when the mode changes.
//! - **Reading shared folders** (`read`): the same three words for « lisent les dossiers partagés », your own agents
//!   only (as before), with explicit (reader, source) pairs under `some` (`reads`). Only agents that can share
//!   (shares.rs `why_not`) read or are read.
//! - **A linked household** (`households`, by id): « ses agents écrivent aux tiens » (`in`: which of YOUR agents
//!   theirs may write to) and « les tiens écrivent aux siens » (`out`: which of THEIR agents yours may write to), each
//!   none | all | only [agents]: the agents named are always the receiving side's. A way works only when both
//!   households allow it: what works is what both rules allow (`effective`), the other side's rule as its bridge last
//!   told it (`theirs`, rules_live.rs: `POST /family/rules`, signed with the link's own key). Until it said, nothing.
//!   Every agent of the sending side writes along a way that works, but for its agents « à l'écart » and its family's
//!   agent: each bridge leaves its own out.
//! - **The family's agent** (`family`, family_agent.rs): the own agents it may ask (« Peut demander à »). It is not one
//!   of « tes agents » for the rules: no rule makes it write, and your agents write to it only along an explicit pair
//!   under `some`; it reads no one's shared folder, and is out of the households' rules both ways.
//! - **À l'écart** (`aside`): agents out of everything (they write to no one, no one writes to them or reads their
//!   folder), whatever the rules.
//!
//! Every gate asks this module (`may_write`, `may_read`, through `allows` / `reads_of` for a config): peers.rs (Bot
//! Mode's tokens, the relay to and from households, the contacts skill), agent_files.rs (`targets`, the one gate of
//! `send_files_to_agent` and `message_agent`, the family's agent's too), agent_messages.rs, family_news.rs,
//! rooms_remote.rs, vault.rs (who else talks to an agent) and shares.rs (its `load` is `reads_of`). `effective_links`
//! and `effective_reads` list what the rules allow, for the listings (contacts, the older apps' screens).
//!
//! `<data>/agent-rules.json` (0600) holds them. Made once at start from peers.json's links and shares.json's reads
//! (`migrate`, no compatibility window: only Alex and his wife were in the beta): own pairs become `some` with the
//! same pairs, reads likewise, the family's agent's links its « Peut demander à », a household's links one rule per
//! way (`only` those agents, `all` when every one of yours is ticked). What it could not keep is said in its
//! `migrated` record and the log. Those two files stay as they were (a bridge rolled back finds them), never read
//! again. A place with neither file is a new account: `DEFAULT_WRITE`, `DEFAULT_READ`.
//!
//! No Python counterpart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::households::SEPARATOR;

pub const FILE: &str = "agent-rules.json";
pub const VERSION: u32 = 1;
/// A new account (Alex 21/11, confirmed): its agents may all write to each other…
pub const DEFAULT_WRITE: Mode = Mode::All;
/// …and none reads another's shared folder.
pub const DEFAULT_READ: Mode = Mode::None;

/// « Aucun, Certains, Tous » for your own agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    None,
    Some,
    All,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Mode> {
        match text.trim() {
            "none" => Some(Mode::None),
            "some" => Some(Mode::Some),
            "all" => Some(Mode::All),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::None => "none",
            Mode::Some => "some",
            Mode::All => "all",
        }
    }
}

/// « Aucun, Tous, Seulement ceux-ci » for one way between two households.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    None,
    All,
    Only,
}

impl Scope {
    pub fn parse(text: &str) -> Option<Scope> {
        match text.trim() {
            "none" => Some(Scope::None),
            "all" => Some(Scope::All),
            "only" => Some(Scope::Only),
            _ => None,
        }
    }
}

/// Which agents of the receiving side a way reaches. `except`: with `all`, the agents its side leaves out (its own
/// « à l'écart » and family's agent, as its bridge says them).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub mode: Scope,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
}

fn sorted(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let set: BTreeSet<String> = items.into_iter().filter(|a| !a.trim().is_empty()).collect();
    set.into_iter().collect()
}

impl Rule {
    pub fn none() -> Rule {
        Rule::default()
    }

    pub fn all() -> Rule {
        Rule { mode: Scope::All, ..Rule::default() }
    }

    pub fn only<S: Into<String>>(agents: impl IntoIterator<Item = S>) -> Rule {
        Rule { mode: Scope::Only, agents: agents.into_iter().map(Into::into).collect(), except: Vec::new() }.tidy()
    }

    /// Sorted, no repeats, `agents` only with `only` (an empty one is `none`), `except` only with `all`: two rules that
    /// allow the same agents are equal, and sign the same.
    pub fn tidy(mut self) -> Rule {
        self.agents = sorted(std::mem::take(&mut self.agents));
        self.except = sorted(std::mem::take(&mut self.except));
        match self.mode {
            Scope::None => {
                self.agents.clear();
                self.except.clear();
            }
            Scope::All => self.agents.clear(),
            Scope::Only => {
                self.except.clear();
                if self.agents.is_empty() {
                    self.mode = Scope::None;
                }
            }
        }
        self
    }

    pub fn is_none(&self) -> bool {
        match self.mode {
            Scope::None => true,
            Scope::Only => self.agents.is_empty(),
            Scope::All => false,
        }
    }

    pub fn contains(&self, agent: &str) -> bool {
        match self.mode {
            Scope::None => false,
            Scope::All => !self.except.iter().any(|e| e == agent),
            Scope::Only => self.agents.iter().any(|a| a == agent),
        }
    }

    /// What both allow.
    pub fn and(&self, other: &Rule) -> Rule {
        match (self.mode, other.mode) {
            (Scope::None, _) | (_, Scope::None) => Rule::none(),
            (Scope::All, Scope::All) => {
                Rule { mode: Scope::All, agents: Vec::new(), except: [&self.except[..], &other.except[..]].concat() }
                    .tidy()
            }
            (Scope::All, Scope::Only) => Rule::only(other.agents.iter().filter(|a| self.contains(a)).cloned()),
            (Scope::Only, _) => Rule::only(self.agents.iter().filter(|a| other.contains(a)).cloned()),
        }
    }

    /// What either allows (« D'accord » adds what they propose to what you allowed).
    pub fn or(&self, other: &Rule) -> Rule {
        match (self.mode, other.mode) {
            (Scope::None, _) => other.clone().tidy(),
            (_, Scope::None) => self.clone().tidy(),
            (Scope::All, Scope::All) => Rule {
                mode: Scope::All,
                agents: Vec::new(),
                except: self.except.iter().filter(|e| other.except.contains(*e)).cloned().collect(),
            }
            .tidy(),
            (Scope::All, Scope::Only) => Rule {
                mode: Scope::All,
                agents: Vec::new(),
                except: self.except.iter().filter(|e| !other.agents.contains(*e)).cloned().collect(),
            }
            .tidy(),
            (Scope::Only, Scope::All) => other.or(self),
            (Scope::Only, Scope::Only) => Rule::only(self.agents.iter().chain(other.agents.iter()).cloned()),
        }
    }

    /// Whether every agent `other` allows, this one allows too. `universe`: every agent the two may name, when known
    /// (needed to compare `all` with a list).
    pub fn covers(&self, other: &Rule, universe: Option<&[String]>) -> bool {
        if other.is_none() {
            return true;
        }
        match (self.mode, other.mode) {
            (Scope::None, _) => false,
            (Scope::All, Scope::All) => self.except.iter().all(|e| other.except.contains(e)),
            (_, Scope::Only) => other.agents.iter().all(|a| self.contains(a)),
            (Scope::Only, Scope::All) => match universe {
                Some(every) => every.iter().filter(|a| other.contains(a)).all(|a| self.contains(a)),
                None => false,
            },
            (_, Scope::None) => true,
        }
    }

    /// A short mark of what it allows: a « Non merci » holds for this very proposal only.
    pub fn fingerprint(&self) -> String {
        let text = serde_json::to_string(&self.clone().tidy()).unwrap_or_default();
        hex::encode(&Sha256::digest(text.as_bytes())[..8])
    }

    /// As it is told to the other household: the agents `eligible` says take part (the others left out of a list, and
    /// named in `except` with `all`).
    fn told(&self, eligible: &[String], every: &[String]) -> Rule {
        match self.mode {
            Scope::None => Rule::none(),
            Scope::All => Rule {
                mode: Scope::All,
                agents: Vec::new(),
                except: every.iter().filter(|a| !eligible.contains(*a)).cloned().chain(self.except.clone()).collect(),
            }
            .tidy(),
            Scope::Only => Rule::only(self.agents.iter().filter(|a| eligible.contains(*a)).cloned()),
        }
    }
}

/// A way between this household and a linked one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// Their agents write to yours (« ses agents écrivent aux tiens »).
    In,
    /// Yours write to theirs (« les tiens écrivent aux siens »).
    Out,
}

impl Way {
    pub fn parse(text: &str) -> Option<Way> {
        match text.trim() {
            "in" => Some(Way::In),
            "out" => Some(Way::Out),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Way::In => "in",
            Way::Out => "out",
        }
    }
}

/// The other side's rules as its bridge last told them, both ways seen from here.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Theirs {
    /// Their rule for their agents writing to yours (the agents named are yours).
    #[serde(rename = "in", default)]
    pub incoming: Rule,
    /// Their rule for yours writing to theirs (the agents named are theirs).
    #[serde(default)]
    pub out: Rule,
    /// They said « Non merci » to what you propose, by way.
    #[serde(default)]
    pub declined_in: bool,
    #[serde(default)]
    pub declined_out: bool,
    /// When their bridge said it.
    #[serde(default)]
    pub at: f64,
}

/// One linked household.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Household {
    /// « Ses agents écrivent aux tiens »: which of yours.
    #[serde(rename = "in", default)]
    pub incoming: Rule,
    /// « Les tiens écrivent aux siens »: which of theirs.
    #[serde(default)]
    pub out: Rule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theirs: Option<Theirs>,
    /// Your « Non merci » to their proposal, by way: its fingerprint (a new proposal asks again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declined_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declined_out: Option<String>,
    /// Their agents (id, name) as their bridge last listed them (`/family/agents`): for the listings, never a gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<(String, String)>,
    /// When your side last changed, and when their bridge last had it (rules_live.rs tells it again until then).
    #[serde(default)]
    pub changed: f64,
    #[serde(default)]
    pub told: f64,
    /// The proposal of theirs the person was last notified of, by way (one notification per proposal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_out: Option<String>,
}

impl Household {
    pub fn mine(&self, way: Way) -> &Rule {
        match way {
            Way::In => &self.incoming,
            Way::Out => &self.out,
        }
    }

    pub fn mine_mut(&mut self, way: Way) -> &mut Rule {
        match way {
            Way::In => &mut self.incoming,
            Way::Out => &mut self.out,
        }
    }

    /// Their rule for that way (none until their bridge said it).
    pub fn theirs_of(&self, way: Way) -> Rule {
        match (&self.theirs, way) {
            (Some(t), Way::In) => t.incoming.clone(),
            (Some(t), Way::Out) => t.out.clone(),
            (None, _) => Rule::none(),
        }
    }

    /// What works that way: what both allow.
    pub fn effective(&self, way: Way) -> Rule {
        self.mine(way).and(&self.theirs_of(way))
    }

    pub fn declined_by_me(&self, way: Way) -> Option<&String> {
        match way {
            Way::In => self.declined_in.as_ref(),
            Way::Out => self.declined_out.as_ref(),
        }
    }

    fn declined_by_me_mut(&mut self, way: Way) -> &mut Option<String> {
        match way {
            Way::In => &mut self.declined_in,
            Way::Out => &mut self.declined_out,
        }
    }

    pub fn declined_by_them(&self, way: Way) -> bool {
        match (&self.theirs, way) {
            (Some(t), Way::In) => t.declined_in,
            (Some(t), Way::Out) => t.declined_out,
            (None, _) => false,
        }
    }

    /// Their agents' ids, as last listed.
    pub fn their_agents(&self) -> Vec<String> {
        self.agents.iter().map(|(id, _)| id.clone()).collect()
    }

    fn notified_mut(&mut self, way: Way) -> &mut Option<String> {
        match way {
            Way::In => &mut self.notified_in,
            Way::Out => &mut self.notified_out,
        }
    }
}

/// The rules of `<data>/agent-rules.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub write: Mode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<(String, String)>,
    #[serde(default)]
    pub read: Mode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aside: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub family: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub households: BTreeMap<String, Household>,
    /// The shared folders' links the sandboxes were last mounted for (shares.rs): a change of the rules mounts them
    /// again once, for the agents concerned only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_reads: Option<Vec<(String, String)>>,
    /// What the migration from the pairs did and could not keep (once, at the first start of this version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated: Option<Value>,
}

fn version() -> u32 {
    VERSION
}

impl Default for Rules {
    /// Everything closed (a file that can't be read gives this, never the new account's defaults).
    fn default() -> Self {
        Rules {
            version: VERSION,
            write: Mode::None,
            pairs: Vec::new(),
            read: Mode::None,
            reads: Vec::new(),
            aside: Vec::new(),
            family: BTreeMap::new(),
            households: BTreeMap::new(),
            applied_reads: None,
            migrated: None,
        }
    }
}

impl Rules {
    /// A new account's: `DEFAULT_WRITE`, `DEFAULT_READ`.
    pub fn new_account() -> Rules {
        Rules { write: DEFAULT_WRITE, read: DEFAULT_READ, ..Rules::default() }
    }

    pub fn is_aside(&self, agent: &str) -> bool {
        self.aside.iter().any(|a| a == agent)
    }

    /// Sorted lists, no repeats, no pair of an agent with itself: two equal rules are equal files.
    pub fn tidy(mut self) -> Rules {
        let pairs: BTreeSet<(String, String)> = self.pairs.into_iter().filter(|(a, b)| a != b).collect();
        self.pairs = pairs.into_iter().collect();
        let reads: BTreeSet<(String, String)> = self.reads.into_iter().filter(|(a, b)| a != b).collect();
        self.reads = reads.into_iter().collect();
        self.aside = sorted(self.aside);
        for targets in self.family.values_mut() {
            *targets = sorted(std::mem::take(targets));
        }
        for household in self.households.values_mut() {
            household.incoming = std::mem::take(&mut household.incoming).tidy();
            household.out = std::mem::take(&mut household.out).tidy();
            if let Some(theirs) = household.theirs.as_mut() {
                theirs.incoming = std::mem::take(&mut theirs.incoming).tidy();
                theirs.out = std::mem::take(&mut theirs.out).tidy();
            }
        }
        self
    }

    /// An agent gone (deleted): out of every list naming it as one of yours.
    pub fn forget_agent(&mut self, agent: &str) {
        self.pairs.retain(|(a, b)| a != agent && b != agent);
        self.reads.retain(|(a, b)| a != agent && b != agent);
        self.aside.retain(|a| a != agent);
        self.family.remove(agent);
        for targets in self.family.values_mut() {
            targets.retain(|t| t != agent);
        }
        for household in self.households.values_mut() {
            if household.incoming.mode == Scope::Only {
                household.incoming.agents.retain(|a| a != agent);
                household.incoming = std::mem::take(&mut household.incoming).tidy();
            }
        }
    }
}

// -- what the rules allow ---------------------------------------------------------------------------------------------

/// One of this place's agents, as the rules see it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Agent {
    pub id: String,
    /// The family's agent (family_agent.rs).
    #[serde(default)]
    pub family: bool,
    /// It can share and read folders (shares.rs `why_not`).
    #[serde(default = "yes")]
    pub can_share: bool,
}

fn yes() -> bool {
    true
}

/// This place's agents and the households linked to it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct World {
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub linked: BTreeSet<String>,
}

impl World {
    /// `shares`: whether each agent can share (reads its profile; not needed for writing).
    pub fn of(config: &Config, shares: bool) -> World {
        let family: BTreeSet<String> = crate::family_agent::owns(config).into_iter().collect();
        let agents = config
            .agents
            .values()
            .map(|a| Agent {
                id: a.name.clone(),
                family: family.contains(&a.name),
                can_share: !shares || crate::shares::why_not(config, a).is_none(),
            })
            .collect();
        let linked = crate::households::Households::new(&crate::households::path_for(config))
            .load()
            .get("households")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        World { agents, linked }
    }

    pub fn agent(&self, id: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// (their agent, household) for another household's agent; None for one of ours.
    fn remote(&self, id: &str) -> Option<(String, String)> {
        if self.agent(id).is_some() {
            return None;
        }
        crate::households::split(id)
    }
}

/// Your agents that take part in the rules: not « à l'écart », not the family's agent.
pub fn eligible(world: &World, rules: &Rules) -> Vec<String> {
    world.agents.iter().filter(|a| !a.family && !rules.is_aside(&a.id)).map(|a| a.id.clone()).collect()
}

/// Whether `from` may write to `to` (an agent of this place by its id, another household's as `agent__household`).
pub fn may_write(world: &World, rules: &Rules, from: &str, to: &str) -> bool {
    if from == to {
        return false;
    }
    match (world.agent(from), world.agent(to)) {
        (Some(a), Some(b)) => {
            if rules.is_aside(&a.id) || rules.is_aside(&b.id) {
                return false;
            }
            if a.family {
                // « Peut demander à »: its list alone (never another family's agent)
                return !b.family && rules.family.get(&a.id).is_some_and(|t| t.contains(&b.id));
            }
            let pair = || rules.pairs.iter().any(|(x, y)| *x == a.id && *y == b.id);
            if b.family {
                return rules.write == Mode::Some && pair();
            }
            match rules.write {
                Mode::None => false,
                Mode::All => true,
                Mode::Some => pair(),
            }
        }
        (Some(a), None) => {
            let Some((agent, household)) = world.remote(to) else { return false };
            !a.family
                && !rules.is_aside(&a.id)
                && world.linked.contains(&household)
                && rules.households.get(&household).is_some_and(|h| h.effective(Way::Out).contains(&agent))
        }
        (None, Some(b)) => {
            let Some((_, household)) = world.remote(from) else { return false };
            opens_to(world, rules, &household, &b.id)
        }
        (None, None) => false,
    }
}

/// Whether the agents of household `household` may write to your `agent` (any of theirs: their bridge checks which).
pub fn opens_to(world: &World, rules: &Rules, household: &str, agent: &str) -> bool {
    let Some(b) = world.agent(agent) else { return false };
    !b.family
        && !rules.is_aside(&b.id)
        && world.linked.contains(household)
        && rules.households.get(household).is_some_and(|h| h.effective(Way::In).contains(&b.id))
}

/// Whether `reader` may read what `source` shares (both of this place, both able to share).
pub fn may_read(world: &World, rules: &Rules, reader: &str, source: &str) -> bool {
    let (Some(r), Some(s)) = (world.agent(reader), world.agent(source)) else { return false };
    if r.id == s.id || !r.can_share || !s.can_share || r.family || rules.is_aside(&r.id) || rules.is_aside(&s.id) {
        return false;
    }
    let pair = || rules.reads.iter().any(|(x, y)| *x == r.id && *y == s.id);
    if s.family {
        return rules.read == Mode::Some && pair();
    }
    match rules.read {
        Mode::None => false,
        Mode::All => true,
        Mode::Some => pair(),
    }
}

/// Whether your side of the rules lets that agent of a linked household take part (yours may write to it, or theirs
/// to one of yours): rooms_remote.rs asks it for a guest of your discussions, their bridge checks its side at each
/// turn.
pub fn my_side_with(world: &World, rules: &Rules, id: &str) -> bool {
    let Some((agent, household)) = world.remote(id) else { return false };
    world.linked.contains(&household)
        && rules.households.get(&household).is_some_and(|h| h.out.contains(&agent) || !h.incoming.is_none())
}

/// The agents of a linked household the listings can name: as its bridge last listed them, and those a rule names.
fn named_there(household: &Household) -> Vec<String> {
    let mut out: BTreeSet<String> = household.their_agents().into_iter().collect();
    out.extend(household.out.agents.iter().cloned());
    out.extend(household.theirs_of(Way::Out).agents);
    out.into_iter().collect()
}

/// Every (writer, recipient) the rules allow, sorted: this place's agents between them, and with the agents of the
/// linked households their bridges listed.
pub fn effective_links(world: &World, rules: &Rules) -> Vec<(String, String)> {
    let mut out = BTreeSet::new();
    for a in &world.agents {
        for b in &world.agents {
            if may_write(world, rules, &a.id, &b.id) {
                out.insert((a.id.clone(), b.id.clone()));
            }
        }
    }
    for (id, household) in &rules.households {
        if !world.linked.contains(id) {
            continue;
        }
        for theirs in named_there(household) {
            let remote = format!("{theirs}{SEPARATOR}{id}");
            for mine in &world.agents {
                if may_write(world, rules, &mine.id, &remote) {
                    out.insert((mine.id.clone(), remote.clone()));
                }
                if household.agents.iter().any(|(a, _)| *a == theirs) && may_write(world, rules, &remote, &mine.id) {
                    out.insert((remote.clone(), mine.id.clone()));
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Every (reader, source) the rules allow, sorted.
pub fn effective_reads(world: &World, rules: &Rules) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in &world.agents {
        for s in &world.agents {
            if may_read(world, rules, &r.id, &s.id) {
                out.push((r.id.clone(), s.id.clone()));
            }
        }
    }
    out.sort();
    out
}

/// Your agents that write to another household's along a way that works (Bot Mode, peers.rs `set_tokens`): every
/// one that takes part, once a way out works with that household.
pub fn household_senders(world: &World, rules: &Rules) -> BTreeSet<String> {
    let open = rules.households.iter().any(|(id, h)| world.linked.contains(id) && !h.effective(Way::Out).is_none());
    if !open {
        return BTreeSet::new();
    }
    eligible(world, rules).into_iter().collect()
}

/// Your agents another household's agents talk to, or that talk to theirs (vault.rs `others_facing_agents`).
pub fn household_facing(world: &World, rules: &Rules) -> BTreeSet<String> {
    let mut out = household_senders(world, rules);
    for (id, household) in &rules.households {
        if !world.linked.contains(id) {
            continue;
        }
        let incoming = household.effective(Way::In);
        out.extend(eligible(world, rules).into_iter().filter(|a| incoming.contains(a)));
    }
    out
}

/// The households whose agents may write to `agent`, or `agent` to theirs (vault.rs `others_names`).
pub fn households_of(world: &World, rules: &Rules, agent: &str) -> Vec<String> {
    let Some(a) = world.agent(agent) else { return Vec::new() };
    if a.family || rules.is_aside(agent) {
        return Vec::new();
    }
    rules
        .households
        .iter()
        .filter(|(id, h)| {
            world.linked.contains(*id) && (!h.effective(Way::Out).is_none() || h.effective(Way::In).contains(agent))
        })
        .map(|(id, _)| id.clone())
        .collect()
}

// -- both households agreeing -----------------------------------------------------------------------------------------

/// Where one way with a household stands, for the screen (« en attente de Chi », « Chi propose », « d'accord »).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WayState {
    pub mine: Rule,
    /// Theirs as their bridge last said it (null: not yet).
    pub theirs: Option<Rule>,
    pub effective: Rule,
    /// `off` | `waiting` (what you allow waits for them) | `proposed` (they propose more than you allow: a card) |
    /// `agreed` | `declined` (they said « Non merci » to what you propose).
    pub state: &'static str,
    pub waiting: bool,
    pub proposed: bool,
    pub declined_by_them: bool,
    pub declined_by_me: bool,
}

/// Every agent a way may name: yours (way in), theirs as listed (way out; None when never listed).
fn universe(world: &World, rules: &Rules, household: &Household, way: Way) -> Option<Vec<String>> {
    match way {
        Way::In => Some(eligible(world, rules)),
        Way::Out => Some(household.their_agents()).filter(|a| !a.is_empty()),
    }
}

pub fn way_state(world: &World, rules: &Rules, id: &str, way: Way) -> WayState {
    let household = rules.households.get(id).cloned().unwrap_or_default();
    let every = universe(world, rules, &household, way);
    let mine = household.mine(way).clone();
    let theirs = household.theirs.as_ref().map(|_| household.theirs_of(way));
    let their_rule = theirs.clone().unwrap_or_default();
    let declined_by_them = household.declined_by_them(way);
    let declined_by_me = household.declined_by_me(way).is_some_and(|fp| *fp == their_rule.fingerprint());
    // what their side leaves out on purpose (its « à l'écart ») never waits
    let wish = mine.and(&Rule { mode: Scope::All, agents: Vec::new(), except: their_rule.except.clone() });
    let waiting = !wish.is_none() && !their_rule.covers(&wish, every.as_deref()) && !declined_by_them;
    let proposed = !their_rule.is_none() && !mine.covers(&their_rule, every.as_deref()) && !declined_by_me;
    let effective = mine.and(&their_rule);
    let state = if proposed {
        "proposed"
    } else if waiting {
        "waiting"
    } else if !effective.is_none() {
        "agreed"
    } else if declined_by_them && !mine.is_none() {
        "declined"
    } else {
        "off"
    };
    WayState { mine, theirs, effective, state, waiting, proposed, declined_by_them, declined_by_me }
}

/// The person sets their own rule for a way with household `id`: what they propose beyond it is answered by that
/// very choice (no card asks them again until they propose something new). True when it changed.
pub fn set_mine(world: &World, rules: &mut Rules, id: &str, way: Way, rule: Rule, now: f64) -> bool {
    let rule = rule.tidy();
    let household = rules.households.entry(id.to_string()).or_default();
    let changed = *household.mine(way) != rule;
    *household.mine_mut(way) = rule;
    if changed {
        household.changed = now;
    }
    let every = universe(world, rules, &rules.households[id], way);
    let household = rules.households.get_mut(id).expect("just made");
    let theirs = household.theirs_of(way);
    if !theirs.is_none() && !household.mine(way).covers(&theirs, every.as_deref()) {
        *household.declined_by_me_mut(way) = Some(theirs.fingerprint());
    }
    changed
}

/// « D'accord » to their proposal that way: what they propose added to what you allow. False: nothing proposed.
pub fn accept(rules: &mut Rules, id: &str, way: Way) -> bool {
    let Some(household) = rules.households.get_mut(id) else { return false };
    let theirs = household.theirs_of(way);
    if theirs.is_none() {
        return false;
    }
    let mine = household.mine(way).or(&theirs);
    *household.mine_mut(way) = mine;
    *household.declined_by_me_mut(way) = None;
    true
}

/// « Non merci » to their proposal that way (this very one: a new proposal asks again). False: nothing proposed.
pub fn decline(rules: &mut Rules, id: &str, way: Way) -> bool {
    let Some(household) = rules.households.get_mut(id) else { return false };
    let theirs = household.theirs_of(way);
    if theirs.is_none() {
        return false;
    }
    *household.declined_by_me_mut(way) = Some(theirs.fingerprint());
    true
}

/// Their proposal that way, when it waits for the person (a card in the Boîte), with its fingerprint.
pub fn pending_proposal(world: &World, rules: &Rules, id: &str, way: Way) -> Option<(Rule, String)> {
    if !world.linked.contains(id) {
        return None;
    }
    let state = way_state(world, rules, id, way);
    let theirs = state.theirs.filter(|_| state.proposed)?;
    let fingerprint = theirs.fingerprint();
    Some((theirs, fingerprint))
}

/// Whether the person was told of that proposal already; marks it told. (One notification per proposal.)
pub fn first_notice(rules: &mut Rules, id: &str, way: Way, fingerprint: &str) -> bool {
    let Some(household) = rules.households.get_mut(id) else { return false };
    let slot = household.notified_mut(way);
    if slot.as_deref() == Some(fingerprint) {
        return false;
    }
    *slot = Some(fingerprint.to_string());
    true
}

// -- what two households tell each other ------------------------------------------------------------------------------

/// What this side tells a linked household (`POST /family/rules`), from this side: for each way, the rule and
/// whether you said « Non merci » to what they propose. Nothing else: no agent names, no other data.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Wire {
    #[serde(default = "version")]
    pub v: u32,
    /// Your agents writing to mine: which of mine (my `in`).
    #[serde(default)]
    pub from_you: Rule,
    /// Mine writing to yours: which of yours (my `out`).
    #[serde(default)]
    pub to_you: Rule,
    /// I said « Non merci » to your agents writing to mine as you propose (your `out`).
    #[serde(default)]
    pub declined_from_you: bool,
    /// I said « Non merci » to mine writing to yours as you propose (your `in`).
    #[serde(default)]
    pub declined_to_you: bool,
    #[serde(default)]
    pub at: f64,
}

/// What this side tells household `id` now.
pub fn wire_for(world: &World, rules: &Rules, id: &str, now: f64) -> Wire {
    let household = rules.households.get(id).cloned().unwrap_or_default();
    let every: Vec<String> = world.agents.iter().map(|a| a.id.clone()).collect();
    let mine_in = household.incoming.told(&eligible(world, rules), &every);
    let declined = |way: Way| household.declined_by_me(way).is_some_and(|fp| *fp == household.theirs_of(way).fingerprint());
    Wire {
        v: VERSION,
        from_you: mine_in,
        to_you: household.out.clone().tidy(),
        declined_from_you: declined(Way::In),
        declined_to_you: declined(Way::Out),
        at: now,
    }
}

/// Their side as their bridge told it, kept for household `id`. True when it changed anything.
pub fn heard(rules: &mut Rules, id: &str, wire: &Wire) -> bool {
    let household = rules.households.entry(id.to_string()).or_default();
    let theirs = Theirs {
        incoming: wire.to_you.clone().tidy(),
        out: wire.from_you.clone().tidy(),
        declined_in: wire.declined_to_you,
        declined_out: wire.declined_from_you,
        at: wire.at,
    };
    let changed = household.theirs.as_ref().map(|t| Theirs { at: wire.at, ..t.clone() }) != Some(theirs.clone());
    household.theirs = Some(theirs);
    changed
}

type HmacSha256 = Hmac<Sha256>;

/// The key that signs what two households tell each other of their rules: from the link's own key (family_seal.rs).
pub fn signing_key(seal: &str) -> Option<[u8; 32]> {
    let base = crate::family_seal::decode_key(seal)?;
    Some(crate::family_seal::hkdf(b"bip-agent-rules-v1", &base, b"rules"))
}

pub fn sign(key: &[u8; 32], wire: &Wire) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC takes any key");
    mac.update(&serde_json::to_vec(wire).unwrap_or_default());
    hex::encode(mac.finalize().into_bytes())
}

pub fn signed_by(key: &[u8; 32], wire: &Wire, signature: &str) -> bool {
    let Ok(given) = hex::decode(signature.trim()) else { return false };
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC takes any key");
    mac.update(&serde_json::to_vec(wire).unwrap_or_default());
    mac.verify_slice(&given).is_ok()
}

// -- what older apps send and read (PUT /v1/agent-links, /v1/agent-shares) -------------------------------------------

/// A way's rule from the agents ticked: `all` when every one `every` names is (and there is one), else `only` those.
fn from_ticked(ticked: BTreeSet<String>, every: &[String]) -> Rule {
    if !every.is_empty() && every.iter().all(|a| ticked.contains(a)) {
        Rule::all()
    } else {
        Rule::only(ticked)
    }
}

/// The pairs an older app sends (the whole list, as « Qui peut écrire à qui » ticked it): your agents' under `some`
/// with exactly those, the family's agent's its « Peut demander à », a household's one rule per way. Returns the
/// households whose rules changed (to tell them).
pub fn from_pairs(world: &World, rules: &mut Rules, wanted: &[(String, String)], now: f64) -> Vec<String> {
    let mut own = Vec::new();
    let mut family: BTreeMap<String, Vec<String>> =
        world.agents.iter().filter(|a| a.family).map(|a| (a.id.clone(), Vec::new())).collect();
    let mut incoming: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (from, to) in wanted {
        match (world.agent(from), world.agent(to)) {
            (Some(a), Some(_)) if a.family => family.entry(a.id.clone()).or_default().push(to.clone()),
            (Some(_), Some(_)) => own.push((from.clone(), to.clone())),
            (Some(_), None) => {
                if let Some((agent, household)) = world.remote(to) {
                    out.entry(household).or_default().insert(agent);
                }
            }
            (None, Some(_)) => {
                if let Some((_, household)) = world.remote(from) {
                    incoming.entry(household).or_default().insert(to.clone());
                }
            }
            (None, None) => {}
        }
    }
    rules.write = Mode::Some;
    rules.pairs = own;
    rules.family = family.into_iter().filter(|(_, t)| !t.is_empty()).collect();
    let mine = eligible(world, rules);
    let mut changed = Vec::new();
    for id in &world.linked {
        let theirs_every = rules.households.get(id).map(Household::their_agents).unwrap_or_default();
        let wanted_in = from_ticked(incoming.remove(id).unwrap_or_default(), &mine);
        let wanted_out = from_ticked(out.remove(id).unwrap_or_default(), &theirs_every);
        let a = set_mine(world, rules, id, Way::In, wanted_in, now);
        let b = set_mine(world, rules, id, Way::Out, wanted_out, now);
        if a || b {
            changed.push(id.clone());
        }
    }
    changed
}

/// What an older app's « Qui peut écrire à qui » shows: your agents' links as the rules allow them, and for a
/// household your side of each way (what you allow, as before, whatever they allow) with the agents its bridge
/// listed.
pub fn own_side_pairs(world: &World, rules: &Rules) -> Vec<(String, String)> {
    let mut out: BTreeSet<(String, String)> = BTreeSet::new();
    for a in &world.agents {
        for b in &world.agents {
            if may_write(world, rules, &a.id, &b.id) {
                out.insert((a.id.clone(), b.id.clone()));
            }
        }
    }
    let mine = eligible(world, rules);
    for (id, household) in &rules.households {
        if !world.linked.contains(id) {
            continue;
        }
        for theirs in named_there(household) {
            let remote = format!("{theirs}{SEPARATOR}{id}");
            if household.out.contains(&theirs) {
                out.extend(mine.iter().map(|a| (a.clone(), remote.clone())));
            }
            if household.agents.iter().any(|(a, _)| *a == theirs) {
                out.extend(mine.iter().filter(|b| household.incoming.contains(b)).map(|b| (remote.clone(), b.clone())));
            }
        }
    }
    out.into_iter().collect()
}

// -- the migration from the pairs -------------------------------------------------------------------------------------

/// What the pairs' files held (None: no such file).
#[derive(Debug, Clone, Default)]
pub struct Before {
    pub links: Option<Vec<(String, String)>>,
    pub reads: Option<Vec<(String, String)>>,
}

/// The rules the pairs give (pure: `at_start` writes them once), and what could not be kept as it was.
pub fn migrate(world: &World, before: &Before) -> (Rules, Vec<String>) {
    let mut notes = Vec::new();
    if before.links.is_none() && before.reads.is_none() {
        notes.push("no pairs before: a new account's rules".to_string());
        return (Rules::new_account(), notes);
    }
    let mut rules = Rules::default();
    let links = before.links.clone().unwrap_or_default();
    let mut incoming: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut senders: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (from, to) in &links {
        match (world.agent(from), world.agent(to)) {
            (Some(a), Some(b)) if a.family && !b.family => {
                rules.family.entry(a.id.clone()).or_default().push(b.id.clone())
            }
            (Some(a), Some(b)) if a.family && b.family => {
                notes.push(format!("{from} → {to} dropped: a family's agent asks no other family's agent"))
            }
            (Some(_), Some(_)) => rules.pairs.push((from.clone(), to.clone())),
            (Some(a), None) => match world.remote(to) {
                Some(_) if a.family => notes.push(format!("{from} → {to} dropped: the family's agent writes to no household")),
                Some((agent, household)) if world.linked.contains(&household) => {
                    out.entry(household.clone()).or_default().insert(agent);
                    senders.entry(household).or_default().insert(a.id.clone());
                }
                _ => notes.push(format!("{from} → {to} dropped: no such agent or household")),
            },
            (None, Some(b)) => match world.remote(from) {
                Some(_) if b.family => notes.push(format!("{from} → {to} dropped: no household writes to the family's agent")),
                Some((_, household)) if world.linked.contains(&household) => {
                    incoming.entry(household).or_default().insert(b.id.clone());
                }
                _ => notes.push(format!("{from} → {to} dropped: no such agent or household")),
            },
            (None, None) => notes.push(format!("{from} → {to} dropped: no agent of this place")),
        }
    }
    rules.write = Mode::Some;
    let mine = eligible(world, &rules);
    for (household, targets) in incoming {
        let entry = rules.households.entry(household.clone()).or_default();
        entry.incoming = from_ticked(targets, &mine);
        notes.push(format!("{household}: their agents write to {} (every one of theirs: the rule names yours)",
            if entry.incoming.mode == Scope::All { "all of yours".to_string() } else { entry.incoming.agents.join(", ") }));
    }
    for (household, targets) in out {
        let entry = rules.households.entry(household.clone()).or_default();
        entry.out = Rule::only(targets);
        let wrote = senders.remove(&household).unwrap_or_default();
        let more: Vec<&String> = mine.iter().filter(|a| !wrote.contains(*a)).collect();
        if !more.is_empty() {
            let more: Vec<&str> = more.iter().map(|a| a.as_str()).collect();
            notes.push(format!("{household}: {} write to {} too now (the rule is per household)",
                more.join(", "), entry.out.agents.join(", ")));
        }
    }
    match &before.reads {
        Some(reads) => {
            rules.read = Mode::Some;
            for (reader, source) in reads {
                match (world.agent(reader), world.agent(source)) {
                    (Some(r), Some(_)) if r.family => {
                        notes.push(format!("{reader} reads {source} dropped: the family's agent reads no one's folder"))
                    }
                    (Some(_), Some(_)) => rules.reads.push((reader.clone(), source.clone())),
                    _ => notes.push(format!("{reader} reads {source} dropped: no such agent")),
                }
            }
        }
        None => rules.read = DEFAULT_READ,
    }
    (rules.tidy(), notes)
}

// -- the file ---------------------------------------------------------------------------------------------------------

/// One read-change-write of the rules at a time in the process.
static WRITING: Mutex<()> = Mutex::new(());

pub fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join(FILE)
}

fn pairs_in(data: &Value, key: &str) -> Vec<(String, String)> {
    data.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|x| x.as_array().filter(|p| p.len() == 2))
                .map(|p| (crate::py::py_str(&p[0]), crate::py::py_str(&p[1])))
                .collect()
        })
        .unwrap_or_default()
}

/// What peers.json and shares.json held (they are never written by this version: peers.json keeps its tokens).
pub fn before_of(config: &Config) -> Before {
    let read = |name: &str, key: &str| -> Option<Vec<(String, String)>> {
        let text = std::fs::read_to_string(PathBuf::from(config.data_dir()).join(name)).ok()?;
        let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Some(pairs_in(&data, key))
    };
    Before { links: read("peers.json", "links"), reads: read("shares.json", "reads") }
}

/// The rules. No file yet: the ones the pairs give (what `at_start` will write). A file that can't be read: everything
/// closed (said in the log), never a new account's defaults.
pub fn load(config: &Config) -> Rules {
    match std::fs::read_to_string(path(config)) {
        Ok(text) => match serde_json::from_str::<Rules>(&text) {
            Ok(rules) => rules,
            Err(exc) => {
                tracing::warn!(error = %exc, "agent-rules.json unreadable: every link closed until it is fixed");
                Rules::default()
            }
        },
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => migrate(&World::of(config, false), &before_of(config)).0,
        Err(exc) => {
            tracing::warn!(error = %exc, "agent-rules.json unreadable: every link closed until it is fixed");
            Rules::default()
        }
    }
}

fn save(config: &Config, rules: &Rules) -> anyhow::Result<()> {
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = file.with_file_name(format!("{FILE}.{}.{n}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let text = serde_json::to_string_pretty(rules)?;
    crate::py::write_with_mode(&tmp.to_string_lossy(), format!("{text}\n").as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// The rules changed by `edit`, written once (tidy). Returns (before, after).
pub fn change(config: &Config, edit: impl FnOnce(&mut Rules)) -> anyhow::Result<(Rules, Rules)> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut before = load(config);
    if !path(config).exists() && before.applied_reads.is_none() {
        // made before `at_start` ran: the folders are mounted for the reads of before
        before.applied_reads = Some(before_of(config).reads.unwrap_or_default());
    }
    let mut after = before.clone();
    edit(&mut after);
    let after = after.tidy();
    if after != before || !path(config).exists() {
        save(config, &after)?;
    }
    Ok((before, after))
}

/// At start, once: the rules made from the pairs (`migrate`), written with what was done, and said in the log.
/// Idempotent: a place that has its rules keeps them.
pub fn at_start(config: &Config) -> anyhow::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    if path(config).exists() {
        return Ok(false);
    }
    let before = before_of(config);
    let (mut rules, notes) = migrate(&World::of(config, false), &before);
    // the folders are mounted for the reads as they were
    rules.applied_reads = Some(before.reads.clone().unwrap_or_default());
    rules.migrated = Some(json!({
        "at": crate::py::time_time(),
        "links": before.links.as_ref().map(Vec::len),
        "reads": before.reads.as_ref().map(Vec::len),
        "notes": notes,
    }));
    save(config, &rules)?;
    tracing::info!(write = rules.write.as_str(), read = rules.read.as_str(), pairs = rules.pairs.len(),
        reads = rules.reads.len(), households = rules.households.len(), "agent rules made from the pairs");
    for note in &notes {
        tracing::info!(note = %note, "agent rules: migration");
    }
    Ok(true)
}

// -- for the gates of the other modules -------------------------------------------------------------------------------

/// Whether `from` may write to `to` on this place (the one gate: `may_write`).
pub fn allows(config: &Config, from: &str, to: &str) -> bool {
    may_write(&World::of(config, false), &load(config), from, to)
}

/// Every link the rules allow on this place (`effective_links`).
pub fn links_of(config: &Config) -> Vec<(String, String)> {
    effective_links(&World::of(config, false), &load(config))
}

/// Every (reader, source) the rules allow on this place (`effective_reads`).
pub fn reads_of(config: &Config) -> Vec<(String, String)> {
    effective_reads(&World::of(config, true), &load(config))
}

/// The agents of this place `agent` may write to.
pub fn local_targets(config: &Config, agent: &str) -> Vec<String> {
    let world = World::of(config, false);
    let rules = load(config);
    world.agents.iter().filter(|b| may_write(&world, &rules, agent, &b.id)).map(|b| b.id.clone()).collect()
}

/// Their agents as their bridge just listed them, kept for the listings (written only when they changed).
pub fn remember_agents(config: &Config, household: &str, agents: &[(String, String)]) {
    if household.is_empty() {
        return;
    }
    let current = load(config);
    if current.households.get(household).map(|h| &h.agents[..]) == Some(agents) {
        return;
    }
    let listed = agents.to_vec();
    if let Err(exc) = change(config, |rules| {
        rules.households.entry(household.to_string()).or_default().agents = listed;
    }) {
        tracing::warn!(household, error = %exc, "their agents' list not kept");
    }
}

/// A household unlinked: its rules go.
pub fn forget_household(config: &Config, household: &str) {
    if let Err(exc) = change(config, |rules| {
        rules.households.remove(household);
    }) {
        tracing::warn!(household, error = %exc, "the household's rules not removed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(ids: &[&str], linked: &[&str]) -> World {
        World {
            agents: ids
                .iter()
                .map(|id| Agent { id: id.to_string(), family: *id == "famille", can_share: !id.starts_with("mac") })
                .collect(),
            linked: linked.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn p(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn chi(incoming: Rule, out: Rule, theirs: Option<(Rule, Rule)>) -> Household {
        Household {
            incoming,
            out,
            theirs: theirs.map(|(i, o)| Theirs { incoming: i, out: o, ..Theirs::default() }),
            agents: vec![("bip".into(), "Bip".into()), ("sum".into(), "Summarizer".into())],
            ..Household::default()
        }
    }

    #[test]
    fn your_agents_none_some_all() {
        let w = world(&["vie", "wellness", "kerfy"], &[]);
        let mut r = Rules::new_account();
        assert_eq!(effective_links(&w, &r).len(), 6, "a new account: every one writes to every other");
        assert!(effective_reads(&w, &r).is_empty(), "and none reads another's folder");
        r.write = Mode::Some;
        r.pairs = p(&[("vie", "wellness"), ("kerfy", "vie")]);
        assert_eq!(effective_links(&w, &r), p(&[("kerfy", "vie"), ("vie", "wellness")]));
        r.write = Mode::None;
        assert!(effective_links(&w, &r).is_empty());
        r.write = Mode::All; // the pairs are kept for « Certains » again
        assert_eq!(r.pairs.len(), 2);
        assert!(may_write(&w, &r, "kerfy", "wellness") && !may_write(&w, &r, "vie", "vie"));
    }

    #[test]
    fn a_new_agent_is_covered_at_once() {
        let mut w = world(&["vie", "wellness"], &[]);
        let r = Rules { read: Mode::All, ..Rules::new_account() };
        w.agents.push(Agent { id: "jardin".into(), family: false, can_share: true });
        assert!(may_write(&w, &r, "jardin", "vie") && may_write(&w, &r, "vie", "jardin"));
        assert!(may_read(&w, &r, "jardin", "wellness") && may_read(&w, &r, "wellness", "jardin"));
    }

    #[test]
    fn aside_is_out_of_everything() {
        let w = world(&["vie", "wellness", "compta", "famille"], &["chi"]);
        let mut r = Rules { write: Mode::All, read: Mode::All, aside: vec!["compta".into()], ..Rules::default() };
        r.family.insert("famille".into(), vec!["vie".into(), "compta".into()]);
        r.households.insert("chi".into(), chi(Rule::all(), Rule::all(), Some((Rule::all(), Rule::all()))));
        for other in ["vie", "wellness", "famille", "bip__chi"] {
            assert!(!may_write(&w, &r, "compta", other) && !may_write(&w, &r, other, "compta"), "{other}");
            assert!(!may_read(&w, &r, "compta", other) && !may_read(&w, &r, other, "compta"), "{other}");
        }
        assert!(may_write(&w, &r, "famille", "vie"));
        // told to Chi: « all » but compta (and the family's agent)
        let told = wire_for(&w, &r, "chi", 1.0);
        assert_eq!(told.from_you, Rule { mode: Scope::All, agents: vec![], except: vec!["compta".into(), "famille".into()] });
    }

    #[test]
    fn the_familys_agent_asks_its_list_only() {
        let w = world(&["vie", "wellness", "famille"], &["chi"]);
        let mut r = Rules::new_account();
        assert!(!may_write(&w, &r, "famille", "vie"), "no rule makes it write");
        assert!(!may_write(&w, &r, "vie", "famille"), "« Tous » is not the family's agent");
        r.family.insert("famille".into(), vec!["vie".into()]);
        assert!(may_write(&w, &r, "famille", "vie") && !may_write(&w, &r, "famille", "wellness"));
        r.write = Mode::Some;
        r.pairs = p(&[("vie", "famille")]);
        assert!(may_write(&w, &r, "vie", "famille"), "an explicit pair under « Certains »");
        r.households.insert("chi".into(), chi(Rule::all(), Rule::all(), Some((Rule::all(), Rule::all()))));
        assert!(!may_write(&w, &r, "famille", "bip__chi") && !may_write(&w, &r, "bip__chi", "famille"));
        r.read = Mode::All;
        assert!(!may_read(&w, &r, "famille", "vie"), "it reads no one's folder");
    }

    #[test]
    fn a_household_way_works_only_when_both_agree() {
        let w = world(&["vie", "wellness"], &["chi"]);
        let mut r = Rules::default();
        // you open Vie to Chi's agents; Chi has said nothing yet
        r.households.insert("chi".into(), chi(Rule::only(["vie"]), Rule::none(), None));
        assert!(!may_write(&w, &r, "bip__chi", "vie"));
        assert_eq!(way_state(&w, &r, "chi", Way::In).state, "waiting");
        // Chi says her agents may write to all of yours: what works is Vie
        heard(&mut r, "chi", &Wire { to_you: Rule::all(), ..Wire::default() });
        assert!(may_write(&w, &r, "bip__chi", "vie") && may_write(&w, &r, "new__chi", "vie"));
        assert!(!may_write(&w, &r, "bip__chi", "wellness"));
        let state = way_state(&w, &r, "chi", Way::In);
        assert_eq!((state.state, state.effective.clone()), ("proposed", Rule::only(["vie"])), "she proposes more");
        assert!(pending_proposal(&w, &r, "chi", Way::In).is_some());
        // « Non merci »: no card any more, she is told
        assert!(decline(&mut r, "chi", Way::In));
        assert!(pending_proposal(&w, &r, "chi", Way::In).is_none());
        assert_eq!(way_state(&w, &r, "chi", Way::In).state, "agreed");
        assert!(wire_for(&w, &r, "chi", 2.0).declined_from_you);
        // the way out: Chi opens her Bip to your agents
        heard(&mut r, "chi", &Wire { to_you: Rule::all(), from_you: Rule::only(["bip"]), ..Wire::default() });
        assert_eq!(way_state(&w, &r, "chi", Way::Out).state, "proposed");
        assert!(accept(&mut r, "chi", Way::Out));
        assert_eq!(way_state(&w, &r, "chi", Way::Out).state, "agreed");
        assert!(may_write(&w, &r, "vie", "bip__chi") && may_write(&w, &r, "wellness", "bip__chi"));
        assert!(!may_write(&w, &r, "vie", "sum__chi"));
        assert_eq!(household_senders(&w, &r), ["vie", "wellness"].map(String::from).into_iter().collect());
        // unlinked: nothing
        let unlinked = world(&["vie", "wellness"], &[]);
        assert!(!may_write(&unlinked, &r, "vie", "bip__chi") && !may_write(&unlinked, &r, "bip__chi", "vie"));
    }

    #[test]
    fn what_they_said_no_to_shows_as_declined() {
        let w = world(&["vie"], &["chi"]);
        let mut r = Rules::default();
        r.households.insert("chi".into(), chi(Rule::none(), Rule::only(["bip"]), None));
        heard(&mut r, "chi", &Wire { declined_from_you: true, ..Wire::default() });
        let state = way_state(&w, &r, "chi", Way::Out);
        assert_eq!((state.state, state.waiting, state.declined_by_them), ("declined", false, true));
    }

    #[test]
    fn rules_combine_like_sets() {
        let every: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let ab = Rule::only(["b", "a", "a"]);
        assert_eq!(ab.agents, ["a", "b"]);
        assert_eq!(Rule::only(Vec::<String>::new()).mode, Scope::None);
        assert_eq!(Rule::all().and(&ab), ab);
        assert_eq!(ab.and(&Rule::only(["b", "c"])), Rule::only(["b"]));
        assert_eq!(ab.or(&Rule::only(["c"])), Rule::only(["a", "b", "c"]));
        assert!(Rule::all().covers(&ab, None) && !ab.covers(&Rule::all(), None));
        assert!(Rule::only(every.clone()).covers(&Rule::all(), Some(&every)));
        let but_c = Rule { mode: Scope::All, agents: vec![], except: vec!["c".into()] };
        assert!(!but_c.contains("c") && but_c.contains("a"));
        assert_eq!(but_c.and(&Rule::only(["a", "c"])), Rule::only(["a"]));
        assert_eq!(but_c.or(&Rule::only(["c"])), Rule::all());
        assert_ne!(ab.fingerprint(), Rule::all().fingerprint());
        assert_eq!(ab.fingerprint(), Rule::only(["a", "b"]).fingerprint());
    }

    #[test]
    fn the_wire_is_signed_with_the_links_own_key() {
        let seal = crate::family_seal::encode_key(&[7u8; 32]);
        let key = signing_key(&seal).unwrap();
        let wire = Wire { v: 1, from_you: Rule::only(["vie"]), at: 12.5, ..Wire::default() };
        let signature = sign(&key, &wire);
        assert!(signed_by(&key, &wire, &signature));
        let other = Wire { from_you: Rule::all(), ..wire.clone() };
        assert!(!signed_by(&key, &other, &signature));
        assert!(!signed_by(&signing_key(&crate::family_seal::encode_key(&[8u8; 32])).unwrap(), &wire, &signature));
        // what crosses: rules only, never names
        let text = serde_json::to_string(&wire).unwrap();
        assert_eq!(text, r#"{"v":1,"from_you":{"mode":"only","agents":["vie"]},"to_you":{"mode":"none"},"declined_from_you":false,"declined_to_you":false,"at":12.5}"#);
    }

    #[test]
    fn migration_from_the_pairs() {
        let w = world(&["vie", "wellness", "kerfy", "famille", "macmini"], &["chi"]);
        let before = Before {
            links: Some(p(&[
                ("vie", "wellness"),
                ("vie", "kerfy"),
                ("wellness", "vie"),
                ("vie", "famille"),
                ("famille", "vie"),
                ("vie", "cuisine__chi"),
                ("bip__chi", "vie"),
                ("sum__chi", "wellness"),
                ("bip__chi", "kerfy"),
                ("bip__chi", "macmini"),
                ("famille", "bip__chi"),
                ("vie", "x__gone"),
            ])),
            reads: Some(p(&[("vie", "wellness"), ("famille", "vie")])),
        };
        let (r, notes) = migrate(&w, &before);
        assert_eq!(r.write, Mode::Some);
        assert_eq!(r.pairs, p(&[("vie", "famille"), ("vie", "kerfy"), ("vie", "wellness"), ("wellness", "vie")]));
        assert_eq!(r.family.get("famille"), Some(&vec!["vie".to_string()]));
        let h = &r.households["chi"];
        assert_eq!(h.incoming, Rule::all(), "every one of yours was ticked: all");
        assert_eq!(h.out, Rule::only(["cuisine"]));
        assert_eq!(r.read, Mode::Some);
        assert_eq!(r.reads, p(&[("vie", "wellness")]));
        for said in ["famille → bip__chi dropped", "vie → x__gone dropped", "famille reads vie dropped",
                     "chi: wellness, kerfy, macmini write to cuisine too now"] {
            assert!(notes.iter().any(|n| n.contains(said)), "{said}: {notes:?}");
        }
        // idempotent: the same pairs give the same rules
        assert_eq!(migrate(&w, &before).0, r);
        // a household's link to one of your agents only: only that one
        let (r, _) = migrate(&w, &Before { links: Some(p(&[("bip__chi", "vie")])), reads: None });
        assert_eq!(r.households["chi"].incoming, Rule::only(["vie"]));
        assert_eq!(r.read, DEFAULT_READ);
        // no file at all: a new account
        let (r, _) = migrate(&w, &Before::default());
        assert_eq!((r.write, r.read), (DEFAULT_WRITE, DEFAULT_READ));
        // a file with no links at all: nothing allowed (as before)
        let (r, _) = migrate(&w, &Before { links: Some(vec![]), reads: None });
        assert!(effective_links(&w, &r).is_empty());
    }

    #[test]
    fn an_older_apps_pairs_become_rules() {
        let w = world(&["vie", "wellness", "famille"], &["chi"]);
        let mut r = Rules::new_account();
        r.households.insert("chi".into(), chi(Rule::none(), Rule::none(), None));
        let changed = from_pairs(&w, &mut r, &p(&[("vie", "wellness"), ("famille", "vie"), ("vie", "bip__chi"),
            ("bip__chi", "vie"), ("bip__chi", "wellness")]), 5.0);
        assert_eq!(changed, ["chi"]);
        assert_eq!((r.write, r.pairs.clone()), (Mode::Some, p(&[("vie", "wellness")])));
        assert_eq!(r.family["famille"], ["vie"]);
        assert_eq!(r.households["chi"].incoming, Rule::all());
        assert_eq!(r.households["chi"].out, Rule::only(["bip"]));
        assert_eq!(r.households["chi"].changed, 5.0);
        // what it shows back: your side, as before
        let shown = own_side_pairs(&w, &r);
        for pair in [("vie", "wellness"), ("famille", "vie"), ("vie", "bip__chi"), ("wellness", "bip__chi"),
                     ("bip__chi", "vie"), ("sum__chi", "wellness")] {
            assert!(shown.contains(&(pair.0.to_string(), pair.1.to_string())), "{pair:?}: {shown:?}");
        }
        assert_eq!(from_pairs(&w, &mut r, &shown, 6.0), Vec::<String>::new(), "sent back as shown: nothing changes");
    }

    #[test]
    fn a_deleted_agent_leaves_every_list() {
        let mut r = Rules {
            pairs: p(&[("vie", "jardin"), ("vie", "kerfy")]),
            reads: p(&[("jardin", "vie")]),
            aside: vec!["jardin".into()],
            ..Rules::default()
        };
        r.family.insert("famille".into(), vec!["jardin".into(), "vie".into()]);
        r.households.insert("chi".into(), chi(Rule::only(["jardin"]), Rule::none(), None));
        r.forget_agent("jardin");
        assert_eq!(r.pairs, p(&[("vie", "kerfy")]));
        assert!(r.reads.is_empty() && r.aside.is_empty());
        assert_eq!(r.family["famille"], ["vie"]);
        assert!(r.households["chi"].incoming.is_none());
    }

    #[test]
    fn a_damaged_file_closes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.outbox.db_path = dir.path().join("outbox.db").to_string_lossy().into_owned();
        std::fs::write(dir.path().join(FILE), "{not json").unwrap();
        assert_eq!(load(&config), Rules::default());
        std::fs::remove_file(dir.path().join(FILE)).unwrap();
        assert_eq!(load(&config).write, DEFAULT_WRITE, "no file and no pairs: a new account");
        std::fs::write(dir.path().join("peers.json"), r#"{"links": [], "tokens": {}}"#).unwrap();
        assert!(at_start(&config).unwrap());
        assert!(!at_start(&config).unwrap(), "once");
        let rules = load(&config);
        assert_eq!(rules.write, Mode::Some);
        assert!(rules.migrated.is_some());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// The cases the apps reuse (contracts/agent-rules/cases.json): each state and what the rules allow.
    #[test]
    fn the_contract_cases() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/agent-rules/cases.json"))
            .expect("contracts/agent-rules/cases.json");
        let cases: Value = serde_json::from_str(&text).unwrap();
        let cases = cases["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let w: World = serde_json::from_value(json!({"agents": case["agents"], "linked": case["linked"]})).unwrap();
            let r: Rules = serde_json::from_value(case["rules"].clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
            let pairs = |v: &Value| -> Vec<(String, String)> {
                v.as_array().unwrap().iter().map(|x| (x[0].as_str().unwrap().into(), x[1].as_str().unwrap().into())).collect()
            };
            assert_eq!(effective_links(&w, &r), pairs(&case["links"]), "{name}: links");
            assert_eq!(effective_reads(&w, &r), pairs(&case["reads"]), "{name}: reads");
            for check in case.get("may_write").and_then(Value::as_array).into_iter().flatten() {
                let (from, to, want) = (check[0].as_str().unwrap(), check[1].as_str().unwrap(), check[2].as_bool().unwrap());
                assert_eq!(may_write(&w, &r, from, to), want, "{name}: {from} → {to}");
            }
            for (id, ways) in case.get("ways").and_then(Value::as_object).into_iter().flatten() {
                for (way, want) in ways.as_object().unwrap() {
                    let state = way_state(&w, &r, id, Way::parse(way).unwrap());
                    assert_eq!(state.state, want.as_str().unwrap(), "{name}: {id} {way}");
                }
            }
        }
    }
}
