//! « Modèle extérieur » (Alex 2026-11-23: « la mémoire partagée et le Carnet sont désactivés par défaut pour un modèle
//! connecté »): an agent whose model is neither one of our European providers nor the person's own. What it is told
//! (its prompt, its tools' results) leaves for a company outside the European Union, or for a server nobody vouches
//! for: agent_shared_memory.rs turns its shared memory and its Carnet off by default, once per provider.
//!
//! Read from the agent's Hermes profile, its `config.yaml` `model:` section (`provider`, `base_url`), else the
//! install's (the hub's, as health::model_of). In this order:
//! 1. **the person's own model** (not outside): an address of this machine or the local network
//!    (archives::records_fill::is_local: localhost, *.local, *.localhost, *.internal, private, loopback and link-local
//!    addresses, fc00::/7), a host name without a dot (`http://sparks:8888/v1`: a name of the local network, never a
//!    provider's), the person's Tailscale network (100.64.0.0/10, `*.ts.net`); or a local provider of Hermes named
//!    without an address ([`LOCAL_PROVIDERS`]: Ollama, LM Studio, llama.cpp, vLLM);
//! 2. **our European providers** (not outside): an address at EUrouter (model_route::pins) or Mistral
//!    ([`EU_HOSTS`]), or `provider: mistral` without one;
//! 3. **everything else is outside**: OpenAI (Codex included), Anthropic, Google, xAI, OpenRouter, DeepSeek, Nous,
//!    GitHub Copilot…, Hermes' `auto`, and any other public address (a `custom` server on the Internet: nobody here
//!    knows who runs it). No `model` section at all: not outside (nothing is sent anywhere yet).
//!
//! No Python counterpart.

use std::path::Path;

use serde_json::Value;

use crate::config::{AgentConfig, Config};

/// Hermes' providers that run on the person's machine when no address is given.
pub const LOCAL_PROVIDERS: [&str; 7] = ["ollama", "lmstudio", "llamacpp", "llama-cpp", "llama.cpp", "vllm", "local"];
/// Hosts (and their subdomains) of our European providers, besides EUrouter's (model_route::pins).
pub const EU_HOSTS: [&str; 1] = ["mistral.ai"];
/// Providers named without an address that are ours.
const EU_PROVIDERS: [&str; 2] = ["mistral", "eurouter"];

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

/// Whether a model's address is one of our European providers'.
pub fn european_address(base_url: &str) -> bool {
    crate::model_route::pins(base_url) || host_of(base_url).is_some_and(|h| EU_HOSTS.iter().any(|d| within(&h, d)))
}

/// The known provider of an address, by its host.
fn known_by_host(host: &str) -> Option<(&'static str, &'static str)> {
    KNOWN.iter().find(|(_, hosts, _)| hosts.iter().any(|d| within(host, d))).map(|(id, _, label)| (*id, *label))
}

/// Where a model runs, from its profile's `provider` and `base_url` (module doc).
pub fn place_of(provider: &str, base_url: &str) -> Place {
    let provider = provider.trim().to_ascii_lowercase();
    let base_url = base_url.trim();
    if base_url.is_empty() {
        if provider.is_empty() {
            return Place::Unknown;
        }
        if LOCAL_PROVIDERS.contains(&provider.as_str()) {
            return Place::Own;
        }
        if EU_PROVIDERS.contains(&provider.as_str()) {
            return Place::Europe;
        }
        let label = KNOWN.iter().find(|(id, _, _)| *id == provider).map(|(_, _, l)| l.to_string());
        let label = label.unwrap_or_else(|| if provider == "auto" { "Hermes (auto)".into() } else { provider.clone() });
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

/// The `model` section of a profile's config (`model: <name>`, Hermes' short form, has no provider): (provider,
/// base_url), None without one.
fn model_section(folder: &Path) -> Option<(String, String)> {
    let data = crate::shares::profile_config(folder);
    let model = data.get("model")?;
    let text = |k: &str| model.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match model {
        Value::Object(_) => Some((text("provider"), text("base_url"))),
        _ => None,
    }
}

/// Where `agent`'s model runs: its profile's model, else the install's (module doc).
pub fn place(config: &Config, agent: &AgentConfig) -> Place {
    // (a test without a Hermes home of its own never reads the machine's ~/.hermes)
    let home_given = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.is_empty());
    let own_home = !config.hermes.home.is_empty() || home_given(&agent.space) || home_given(&agent.host_home);
    let own = (!cfg!(test) || own_home).then(|| model_section(&crate::agents_admin::profile_folder(config, agent)));
    let own = own.flatten();
    let hub = (!cfg!(test) || !config.hermes.home.is_empty()).then(|| crate::agents_admin::install_home(config));
    let (provider, base_url) = match own {
        Some(found) if !found.0.trim().is_empty() || !found.1.trim().is_empty() => found,
        _ => match hub.and_then(|home| model_section(Path::new(&home))) {
            Some(found) => found,
            None => return Place::Unknown,
        },
    };
    place_of(&provider, &base_url)
}

/// `Some((provider, label))` when `agent` is on an outside model.
pub fn outside(config: &Config, agent: &AgentConfig) -> Option<(String, String)> {
    match place(config, agent) {
        Place::Outside { provider, label } => Some((provider, label)),
        _ => None,
    }
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
