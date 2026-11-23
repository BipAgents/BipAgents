//! « Modèle extérieur » (Alex 2026-11-23: « la mémoire partagée et le Carnet sont désactivés par défaut pour un modèle
//! connecté »): an agent whose model is neither one of our European providers nor the person's own. What it is told
//! (its prompt, its tools' results) leaves for a company outside the European Union, or for a server nobody vouches
//! for: agent_shared_memory.rs turns its shared memory, Carnet, Health and links to the other agents off by default,
//! once per provider. **In doubt, it protects** (coordinator 23/11, review C1): a model it cannot tell counts as outside.
//!
//! Read from the agent's Hermes profile's `config.yaml` (else the install's, the hub's, as health::model_of): every
//! model Hermes may send the prompt to (review C7), **one outside is enough**: `model` (the main one), `fallback_model`
//! and `fallback_providers` (Hermes switches to them mid-conversation), `auxiliary.<task>` (compression, the background
//! review…: `auto` / `main` / nothing is the main model), `delegation` and `smart_model_routing.cheap_model`. Each, in
//! this order:
//! 1. **a provider Hermes knows by name** ([`KNOWN`]: OpenAI, Codex, Anthropic, Google, xAI, DeepSeek…) is that
//!    provider whatever address is written beside it (Hermes ignores it for them, review R3); not `custom`,
//!    `openrouter`, nor a local provider;
//! 2. **the person's own model** (not outside): an address of this machine or the local network
//!    (archives::records_fill::is_local: localhost, *.local, *.localhost, *.internal, private, loopback and link-local
//!    addresses, fc00::/7), a host name without a dot (`http://sparks:8888/v1`: a name of the local network, never a
//!    provider's), the person's Tailscale network (100.64.0.0/10, `*.ts.net`); or a local provider of Hermes named
//!    without an address ([`LOCAL_PROVIDERS`]: Ollama, LM Studio, llama.cpp, vLLM) — but an Ollama « cloud » model
//!    (`…:cloud`, `…-cloud`) runs at Ollama's: outside;
//! 3. **our European providers** (not outside): an address at EUrouter (`eurouter.ai` and its subdomains, exactly:
//!    review C2) or Mistral (`mistral.ai`), or `provider: mistral` without one;
//! 4. **everything else is outside**: OpenRouter, Hermes' `auto`, any other public address (a `custom` server on the
//!    Internet: nobody here knows who runs it), and `custom` without its address (Hermes v0.23.1 then goes to the
//!    profile's `CUSTOM_BASE_URL`, else `OPENROUTER_BASE_URL`, else OpenRouter: `runtime_provider_backends.py`
//!    `_resolve_openrouter_runtime`; the first two are read, their addresses only, never a key);
//! 5. **nothing readable** (no `model` at all, a file being rewritten): [`Place::Unknown`], which the protection treats
//!    as outside when it has nothing noted yet, and never as « back on our providers ».
//!
//! No Python counterpart.

use std::path::Path;

use serde_json::Value;

use crate::config::{AgentConfig, Config};

/// Hermes' providers that run on the person's machine when no address is given.
pub const LOCAL_PROVIDERS: [&str; 7] = ["ollama", "lmstudio", "llamacpp", "llama-cpp", "llama.cpp", "vllm", "local"];
/// Hosts (and their subdomains) of our European providers: exactly these, never a label of another name.
pub const EU_HOSTS: [&str; 2] = ["eurouter.ai", "mistral.ai"];
/// Providers named without an address that are ours.
const EU_PROVIDERS: [&str; 2] = ["mistral", "eurouter"];
/// The provider id of a model it cannot tell ([`Place::Unknown`] once the protection applies).
pub const UNKNOWN: &str = "unknown";

/// Known providers: (Hermes' provider id or a preset's, its hosts, the name shown in the apps).
const KNOWN: [(&str, &[&str], &str); 20] = [
    ("openai", &["api.openai.com", "openai.com"], "OpenAI"),
    ("openai-codex", &["chatgpt.com"], "OpenAI"),
    ("codex", &[], "OpenAI"),
    ("anthropic", &["api.anthropic.com", "anthropic.com"], "Anthropic"),
    ("gemini", &["generativelanguage.googleapis.com", "googleapis.com"], "Google"),
    ("google", &[], "Google"),
    ("xai", &["api.x.ai", "x.ai"], "xAI"),
    ("openrouter", &["openrouter.ai"], "OpenRouter"),
    ("deepseek", &["api.deepseek.com", "deepseek.com"], "DeepSeek"),
    ("nous", &["nousresearch.com"], "Nous Research"),
    ("copilot", &["githubcopilot.com"], "GitHub Copilot"),
    ("github-copilot", &[], "GitHub Copilot"),
    ("groq", &["api.groq.com", "groq.com"], "Groq"),
    ("together", &["api.together.xyz", "together.xyz", "together.ai"], "Together AI"),
    ("fireworks", &["fireworks.ai"], "Fireworks AI"),
    ("perplexity", &["perplexity.ai"], "Perplexity"),
    ("moonshot", &["moonshot.ai", "moonshot.cn"], "Moonshot AI"),
    ("zai", &["z.ai", "bigmodel.cn"], "Z.ai"),
    ("minimax", &["minimax.io", "minimaxi.com"], "MiniMax"),
    ("huggingface", &["huggingface.co"], "Hugging Face"),
];

/// Where an agent's model runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// The person's own machine or network (the Spark, Ollama, llama.cpp…).
    Own,
    /// One of our European providers (EUrouter, Mistral).
    Europe,
    /// Outside: `provider` the id kept to apply the defaults once per provider, `label` the name the apps show.
    Outside { provider: String, label: String },
    /// No model set anywhere.
    Unknown,
}

/// The host of a model's address, lowercased (brackets of an IPv6 address left out).
fn host_of(base_url: &str) -> Option<String> {
    let url = url::Url::parse(base_url.trim()).ok()?;
    let host = url.host_str()?.trim_matches(['[', ']']).to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether `host` is `domain` or one of its subdomains.
fn within(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// Whether a model's address is the person's own: this machine, the local network, their Tailscale network.
pub fn own_address(base_url: &str) -> bool {
    let Some(host) = host_of(base_url) else { return false };
    if crate::archives::records_fill::is_local(base_url) || (!host.contains('.') && !host.contains(':')) {
        return true; // (an IPv6 address has colons and no dot: is_local said)
    }
    if within(&host, "ts.net") {
        return true;
    }
    // Tailscale's addresses: 100.64.0.0/10
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64,
        Err(_) => false,
    }
}

/// Whether a model's address is one of our European providers' (the exact hosts and their subdomains).
pub fn european_address(base_url: &str) -> bool {
    host_of(base_url).is_some_and(|h| EU_HOSTS.iter().any(|d| within(&h, d)))
}

/// The known provider of an address, by its host.
fn known_by_host(host: &str) -> Option<(&'static str, &'static str)> {
    KNOWN.iter().find(|(_, hosts, _)| hosts.iter().any(|d| within(host, d))).map(|(id, _, label)| (*id, *label))
}

/// Whether a model's name is one of Ollama's cloud models (`gpt-oss:120b-cloud`, `deepseek-v3.1:671b-cloud`,
/// `kimi-k2:cloud`): served from Ollama's own servers even through a local Ollama.
pub fn cloud_model(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name.rsplit_once(':').is_some_and(|(_, tag)| tag == "cloud" || tag.ends_with("-cloud"))
}

/// Where a model runs, from its profile's `provider` and `base_url` (module doc).
pub fn place_of(provider: &str, base_url: &str) -> Place {
    let provider = provider.trim().to_ascii_lowercase();
    let base_url = base_url.trim();
    // 1. a provider Hermes knows by name: that provider, whatever address is written beside it
    if provider != "openrouter" {
        if let Some((id, _, label)) = KNOWN.iter().find(|(id, _, _)| *id == provider) {
            return Place::Outside { provider: id.to_string(), label: label.to_string() };
        }
    }
    if base_url.is_empty() {
        if provider.is_empty() {
            return Place::Unknown;
        }
        if provider == "custom" {
            // (`model_place` reads where Hermes would go first: the profile's CUSTOM_BASE_URL, OPENROUTER_BASE_URL)
            return Place::Outside { provider: "custom".into(), label: "OpenRouter".into() };
        }
        if LOCAL_PROVIDERS.contains(&provider.as_str()) {
            return Place::Own;
        }
        if EU_PROVIDERS.contains(&provider.as_str()) {
            return Place::Europe;
        }
        let label = if provider == "auto" { "Hermes (auto)".to_string() } else { provider.clone() };
        let label = if provider == "openrouter" { "OpenRouter".to_string() } else { label };
        return Place::Outside { provider, label };
    }
    if own_address(base_url) {
        return Place::Own;
    }
    if european_address(base_url) {
        return Place::Europe;
    }
    let host = host_of(base_url).unwrap_or_default();
    match known_by_host(&host) {
        Some((id, label)) => Place::Outside { provider: id.into(), label: label.into() },
        // a server nobody here knows: its host is its name (and the key the defaults are applied once for)
        None if !host.is_empty() => Place::Outside { provider: format!("custom:{host}"), label: host },
        None => Place::Outside { provider: format!("custom:{provider}"), label: provider },
    }
}

/// One model of a profile: where Hermes may send the prompt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Endpoint {
    pub provider: String,
    pub base_url: String,
    pub model: String,
}

impl Endpoint {
    fn of(value: &Value) -> Option<Endpoint> {
        let text = |k: &str| value.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        value.is_object().then(|| Endpoint { provider: text("provider"), base_url: text("base_url"), model: text("model") })
    }
}

/// A value of the profile's `.env` (the addresses Hermes falls back to for `custom`: never a key).
fn env_value(folder: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(folder.join(".env")).ok()?;
    text.lines().rev().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        let name = name.trim();
        let name = name.strip_prefix("export ").map(str::trim).unwrap_or(name);
        (name == key).then(|| value.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty())
    })
}

/// Where one model runs (module doc): its provider and address, an Ollama cloud model, `custom` without its address
/// followed where Hermes goes (`folder`'s `.env`).
pub fn model_place(endpoint: &Endpoint, folder: &Path) -> Place {
    let provider = endpoint.provider.trim().to_ascii_lowercase();
    if provider == "custom" && endpoint.base_url.trim().is_empty() {
        for key in ["CUSTOM_BASE_URL", "OPENROUTER_BASE_URL"] {
            if let Some(url) = env_value(folder, key) {
                return model_place(&Endpoint { base_url: url, ..endpoint.clone() }, folder);
            }
        }
    }
    match place_of(&provider, &endpoint.base_url) {
        Place::Own if cloud_model(&endpoint.model) => {
            Place::Outside { provider: "ollama-cloud".into(), label: "Ollama Cloud".into() }
        }
        place => place,
    }
}

/// Every model of a profile's config that the prompt may go to (module doc): the main one first. An empty main model
/// when `model` is missing or isn't a mapping.
pub fn endpoints(data: &serde_json::Map<String, Value>) -> Vec<Endpoint> {
    let mut out = Vec::new();
    let main = match data.get("model") {
        Some(Value::Object(_)) => data.get("model").and_then(Endpoint::of).unwrap_or_default(),
        Some(Value::String(name)) => Endpoint { model: name.clone(), ..Endpoint::default() },
        _ => Endpoint::default(),
    };
    let main = Endpoint {
        model: if main.model.is_empty() {
            data.get("model").and_then(|m| m.get("default")).and_then(Value::as_str).unwrap_or("").to_string()
        } else {
            main.model
        },
        ..main
    };
    out.push(main);
    // what stands for the main model in a task's or a helper's own entry
    let others = |e: Endpoint| -> Option<Endpoint> {
        let p = e.provider.to_ascii_lowercase();
        let same = (p.is_empty() || p == "auto" || p == "main") && e.base_url.is_empty();
        (!same).then_some(e)
    };
    if let Some(Value::Array(items)) = data.get("fallback_providers") {
        out.extend(items.iter().filter_map(Endpoint::of).filter_map(others));
    }
    for key in ["fallback_model", "delegation"] {
        if let Some(e) = data.get(key).and_then(Endpoint::of).and_then(others) {
            out.push(e);
        }
    }
    if let Some(Value::Object(tasks)) = data.get("auxiliary") {
        out.extend(tasks.values().filter_map(Endpoint::of).filter_map(others));
    }
    if let Some(cheap) = data.get("smart_model_routing").and_then(|r| r.get("cheap_model")).and_then(Endpoint::of) {
        if data.get("smart_model_routing").and_then(|r| r.get("enabled")).and_then(Value::as_bool) != Some(false) {
            out.extend(others(cheap));
        }
    }
    out
}

/// Where a profile's models run, together (module doc): the first outside one (the main model first); else the main
/// model's place.
pub fn profile_place(data: &serde_json::Map<String, Value>, folder: &Path) -> Place {
    let all = endpoints(data);
    let places: Vec<Place> = all.iter().map(|e| model_place(e, folder)).collect();
    if let Some(outside) = places.iter().find(|p| matches!(p, Place::Outside { .. })) {
        return outside.clone();
    }
    places.into_iter().next().unwrap_or(Place::Unknown)
}

/// Whether a profile's config names where its model runs (a `model` with a provider or an address; `model: <name>`,
/// Hermes' short form, takes the install's).
fn names_a_model(data: &serde_json::Map<String, Value>) -> bool {
    endpoints(data).first().is_some_and(|m| !m.provider.is_empty() || !m.base_url.is_empty())
}

/// Where `agent`'s model runs: its profile's models, else the install's (module doc).
pub fn place(config: &Config, agent: &AgentConfig) -> Place {
    // (a unit test without any Hermes home of its own never reads the machine's ~/.hermes: its agents are the
    // person's own there; a test that wants a model gives it a home)
    let home_given = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.is_empty());
    let own_home = !config.hermes.home.is_empty() || home_given(&agent.space) || home_given(&agent.host_home);
    if cfg!(test) && !own_home {
        return Place::Own;
    }
    let folder = crate::agents_admin::profile_folder(config, agent);
    let own = crate::shares::profile_config(&folder);
    if names_a_model(&own) {
        return profile_place(&own, &folder);
    }
    if cfg!(test) && config.hermes.home.is_empty() {
        return Place::Unknown;
    }
    let home = std::path::PathBuf::from(crate::agents_admin::install_home(config));
    let hub = crate::shares::profile_config(&home);
    if names_a_model(&hub) {
        return profile_place(&hub, &folder);
    }
    Place::Unknown
}

/// Where `agent`'s model will run once `change` is written into its profile's `model` section (review R2: the
/// protection is applied before a model route writes the model). `change` gets the section (a mapping, made empty when
/// missing). `Some((provider, label))` when that is outside.
pub fn outside_after(
    config: &Config,
    agent: &AgentConfig,
    change: impl FnOnce(&mut serde_json::Map<String, Value>),
) -> Option<(String, String)> {
    let folder = crate::agents_admin::profile_folder(config, agent);
    let mut data = crate::shares::profile_config(&folder);
    if !names_a_model(&data) && !(cfg!(test) && config.hermes.home.is_empty()) {
        let hub = crate::shares::profile_config(Path::new(&crate::agents_admin::install_home(config)));
        if let Some(model) = hub.get("model") {
            data.insert("model".into(), model.clone());
        }
    }
    let mut model = match data.get("model") {
        Some(Value::Object(map)) => map.clone(),
        _ => serde_json::Map::new(),
    };
    change(&mut model);
    data.insert("model".into(), Value::Object(model));
    match profile_place(&data, &folder) {
        Place::Outside { provider, label } => Some((provider, label)),
        _ => None,
    }
}

/// The install's model (the hub's `model` section), for « back to the default »: None without one.
pub fn install_model(config: &Config) -> Option<serde_json::Map<String, Value>> {
    let hub = crate::shares::profile_config(Path::new(&crate::agents_admin::install_home(config)));
    match hub.get("model") {
        Some(Value::Object(map)) => Some(map.clone()),
        _ => None,
    }
}

/// `Some((provider, label))` when `agent` is on an outside model.
pub fn outside(config: &Config, agent: &AgentConfig) -> Option<(String, String)> {
    match place(config, agent) {
        Place::Outside { provider, label } => Some((provider, label)),
        _ => None,
    }
}

/// What the apps say for a model it cannot tell, in the person's language.
pub fn unknown_label(config: &Config) -> String {
    match crate::notebook::language(config).as_str() {
        "en" => "a model BipAgents can't identify",
        "de" => "ein Modell, das BipAgents nicht erkennt",
        "es" => "un modelo que BipAgents no reconoce",
        "nl" => "een model dat BipAgents niet herkent",
        _ => "un modèle que BipAgents ne reconnaît pas",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outside(provider: &str, base_url: &str) -> Option<(String, String)> {
        match place_of(provider, base_url) {
            Place::Outside { provider, label } => Some((provider, label)),
            _ => None,
        }
    }

    /// The definition, case by case (module doc).
    #[test]
    fn an_outside_model_is_neither_ours_nor_the_person_s() {
        // the person's own
        for (provider, url) in [
            ("custom", "http://192.168.1.123:8888/v1"),
            ("custom", "http://sparks:8888/v1"),
            ("custom", "http://gpu:8888/v1"),
            ("custom", "http://localhost:11434/v1"),
            ("custom", "http://127.0.0.1:8080/v1"),
            ("custom", "http://[::1]:8080/v1"),
            ("custom", "http://gx10-0430.local:8888/v1"),
            ("custom", "http://spark.tail1234.ts.net:8888/v1"),
            ("custom", "http://100.91.112.69:8888/v1"),
            ("ollama", ""),
            ("lmstudio", ""),
            ("llamacpp", ""),
            ("ollama", "http://10.0.0.5:11434"),
        ] {
            assert_eq!(place_of(provider, url), Place::Own, "{provider} {url}");
        }
        // ours, in Europe
        for (provider, url) in [
            ("custom", "https://api.eurouter.ai/api/v1"),
            ("custom", "https://api.mistral.ai/v1"),
            ("mistral", ""),
            ("custom", "https://eu.api.mistral.ai/v1"),
        ] {
            assert_eq!(place_of(provider, url), Place::Europe, "{provider} {url}");
        }
        // outside, with the name the apps show
        for (provider, url, id, label) in [
            ("openai-codex", "", "openai-codex", "OpenAI"),
            ("openai", "", "openai", "OpenAI"),
            ("anthropic", "", "anthropic", "Anthropic"),
            ("gemini", "", "gemini", "Google"),
            ("xai", "", "xai", "xAI"),
            ("openrouter", "", "openrouter", "OpenRouter"),
            ("nous", "", "nous", "Nous Research"),
            ("copilot", "", "copilot", "GitHub Copilot"),
            ("custom", "https://api.openai.com/v1", "openai", "OpenAI"),
            ("custom", "https://openrouter.ai/api/v1", "openrouter", "OpenRouter"),
            ("custom", "https://api.deepseek.com/v1", "deepseek", "DeepSeek"),
            ("custom", "https://generativelanguage.googleapis.com/v1beta/openai", "gemini", "Google"),
            ("custom", "https://api.x.ai/v1", "xai", "xAI"),
            ("custom", "https://llm.example.org/v1", "custom:llm.example.org", "llm.example.org"),
            ("auto", "", "auto", "Hermes (auto)"),
            ("somethingnew", "", "somethingnew", "somethingnew"),
            // a router's name in a path or a subdomain of someone else is not EUrouter
            ("custom", "https://proxy.example.com/eurouter.ai/v1", "custom:proxy.example.com", "proxy.example.com"),
            // a public address in 100.x outside Tailscale's block
            ("custom", "http://100.200.1.1:8888/v1", "custom:100.200.1.1", "100.200.1.1"),
        ] {
            assert_eq!(outside(provider, url), Some((id.to_string(), label.to_string())), "{provider} {url}");
        }
        assert_eq!(place_of("", ""), Place::Unknown);
        // a custom server without its address: Hermes goes to OpenRouter (review C1)
        assert_eq!(outside("custom", ""), Some(("custom".into(), "OpenRouter".into())));
        // a provider Hermes knows by name, whatever address is beside it (review R3)
        assert_eq!(outside("openai", "http://127.0.0.1:4000/v1"), Some(("openai".into(), "OpenAI".into())));
        assert_eq!(outside("anthropic", "https://api.eurouter.ai/api/v1").map(|o| o.1), Some("Anthropic".into()));
        assert_eq!(outside("openrouter", ""), Some(("openrouter".into(), "OpenRouter".into())));
        assert_eq!(place_of("openrouter", "http://127.0.0.1:4000/v1"), Place::Own, "openrouter's name with a proxy at home");
    }

    /// Review 23/11 (red): a host with a label « eurouter » is not EUrouter (model_route::pins is right for its own use,
    /// the routing field, not for « is this European »): a proxy or tunnel anywhere may be named so.
    #[test]
    fn review_a_host_named_eurouter_elsewhere_is_outside() {
        for url in [
            "https://eurouter.proxy.example.com/v1",
            "https://api.eurouter.ai.example.net/v1",
            "https://eurouter.ngrok.app/v1",
        ] {
            assert!(outside("custom", url).is_some(), "{url}: taken for EUrouter");
        }
        assert_eq!(place_of("custom", "https://api.eurouter.ai/api/v1"), Place::Europe);
    }

    fn yaml(text: &str) -> serde_json::Map<String, Value> {
        match crate::agents_admin::pyyaml::to_json(&crate::agents_admin::pyyaml::load(text).unwrap()) {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        }
    }

    /// One outside model is enough: the fallbacks, the tasks' models, delegation (review C7); an Ollama cloud model
    /// (review R3); `custom` without its address followed where Hermes goes (review C1).
    #[test]
    fn every_model_the_prompt_may_go_to_counts() {
        let dir = tempfile::tempdir().unwrap();
        let spark = "model:\n  provider: custom\n  base_url: http://192.168.1.123:8888/v1\n  default: qwen\n";
        assert_eq!(profile_place(&yaml(spark), dir.path()), Place::Own);
        for (extra, label) in [
            ("fallback_providers:\n  - provider: openrouter\n    model: x/y\n", "OpenRouter"),
            ("fallback_model:\n  provider: anthropic\n  model: claude\n", "Anthropic"),
            ("auxiliary:\n  compression:\n    provider: custom\n    base_url: https://api.openai.com/v1\n", "OpenAI"),
            ("delegation:\n  provider: gemini\n  model: g\n", "Google"),
            ("smart_model_routing:\n  enabled: true\n  cheap_model:\n    provider: openrouter\n    model: x\n", "OpenRouter"),
        ] {
            let place = profile_place(&yaml(&format!("{spark}{extra}")), dir.path());
            assert!(matches!(&place, Place::Outside { label: l, .. } if l == label), "{extra}: {place:?}");
        }
        // the tasks on the main model (Hermes' defaults) change nothing
        let defaults = format!("{spark}auxiliary:\n  vision:\n    provider: auto\n    model: ''\n  compression:\n    provider: main\ndelegation:\n  provider: ''\n  model: ''\n");
        assert_eq!(profile_place(&yaml(&defaults), dir.path()), Place::Own);
        // an Ollama cloud model through the local Ollama
        for name in ["gpt-oss:120b-cloud", "kimi-k2:cloud"] {
            let text = format!("model:\n  provider: ollama\n  default: {name}\n");
            assert!(matches!(profile_place(&yaml(&text), dir.path()), Place::Outside { .. }), "{name}");
        }
        assert_eq!(profile_place(&yaml("model:\n  provider: ollama\n  default: qwen3:32b\n"), dir.path()), Place::Own);
        // custom without its address: the profile's CUSTOM_BASE_URL, then OPENROUTER_BASE_URL, then OpenRouter
        let custom = yaml("model:\n  provider: custom\n  default: x\n");
        assert!(matches!(profile_place(&custom, dir.path()), Place::Outside { label, .. } if label == "OpenRouter"));
        std::fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-not-read\nCUSTOM_BASE_URL=http://192.168.1.20:11434/v1\n").unwrap();
        assert_eq!(profile_place(&custom, dir.path()), Place::Own);
        std::fs::write(dir.path().join(".env"), "OPENROUTER_BASE_URL=https://api.eurouter.ai/api/v1\n").unwrap();
        assert_eq!(profile_place(&custom, dir.path()), Place::Europe);
        assert_eq!(profile_place(&yaml(""), dir.path()), Place::Unknown);
    }

    fn config_in(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k".repeat(40));
        config.hermes.home = dir.join("hub").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn the_agent_s_profile_decides_else_the_hub_s() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        let agent = AgentConfig { name: "coach".into(), ..Default::default() };
        let profile = crate::agents_admin::profile_folder(&config, &agent);
        std::fs::create_dir_all(&profile).unwrap();
        assert_eq!(place(&config, &agent), Place::Unknown, "nothing set");
        std::fs::write(dir.path().join("hub/config.yaml"), "model:\n  provider: openai-codex\n  default: gpt-6.1\n").unwrap();
        assert_eq!(super::outside(&config, &agent), Some(("openai-codex".into(), "OpenAI".into())), "the hub's");
        std::fs::write(
            profile.join("config.yaml"),
            "model:\n  provider: custom\n  base_url: https://api.eurouter.ai/api/v1\n  default: glm-5.3\n",
        )
        .unwrap();
        assert_eq!(place(&config, &agent), Place::Europe, "its own");
        std::fs::write(profile.join("config.yaml"), "model: gpt-6.1\n").unwrap();
        assert_eq!(super::outside(&config, &agent).map(|o| o.1), Some("OpenAI".into()), "short form: the hub's provider");
    }
}
