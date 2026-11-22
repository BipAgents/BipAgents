//! Hosts never approved as a place an agent sends to, and never reached by an agent's programs (22/11, passation-pro
//! briefs/enquete-antibot-1122.md): the hosts that serve bot checks, and captcha-solving services.
//!
//! On 22/11 a background task of GLM's set out to pass a shop's Cloudflare wall with a headless Playwright through the
//! proxy, and a card « Quotidien veut envoyer des données à challenges.cloudflare.com » came up, which a bench's
//! script accepted. A person could have said yes without knowing what it was. Now:
//!
//! - **never a card, never validated**: `sandbox_sites::ask` answers at once (`Asked::Never`, no card), `add` and the
//!   `sites` of `connector_register` / `connector_update` refuse them (a `*.` covering one too), an old card's
//!   « Autoriser » leaves them out, and the proxy's file never lists them as validated;
//! - **the proxy** (assets/sandbox/egress_proxy.py `NEVER_CHECKS`, `NEVER_SOLVERS`, the same lists: a test reads them
//!   there) refuses them to the sandboxes' programs and to the connectors, with a reason that says what to do. The
//!   agent's visible browser still reaches the checks' hosts (Alex, 22/11: the browser gets past a check by itself
//!   when it can; one that still blocks is handed to the person, browser_mcp.rs), never a solving service;
//! - the benches' approvers never say yes to them (scripts/token-lab/labkit/never.py reads the proxy's lists).
//!
//! Each name covers its subdomains. Lists of hosts, not of paths: Google's reCAPTCHA on www.google.com/recaptcha or
//! www.gstatic.com/recaptcha can't be told from Google itself here (recaptcha.net, its other home, is listed).

/// What a host is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Never {
    /// It serves bot checks (a captcha, « verify you are human »): the person's to pass, in the agent's browser.
    Check,
    /// A captcha-solving service: never used.
    Solver,
}

/// The bot checks' own hosts (Cloudflare's Turnstile and challenge platform, hCaptcha, reCAPTCHA's other home,
/// DataDome, PerimeterX / HUMAN, Arkose Labs / FunCaptcha, GeeTest, AWS WAF's captcha, Friendly Captcha, MTCaptcha;
/// review 1 R8: PerimeterX's collectors, Friendly Captcha's EU home, Yandex SmartCaptcha, Tencent's captcha — each
/// checked in the DNS on 22/11).
pub const CHECK_HOSTS: [&str; 23] = [
    "challenges.cloudflare.com",
    "hcaptcha.com",
    "recaptcha.net",
    "captcha-delivery.com",
    "api-js.datadome.co",
    "perimeterx.net",
    "px-cloud.net",
    "px-cdn.net",
    "arkoselabs.com",
    "funcaptcha.com",
    "geetest.com",
    "geevisit.com",
    "awswaf.com",
    "friendlycaptcha.com",
    "frcapi.com",
    "mtcaptcha.com",
    "pxchk.net",
    "px-client.net",
    "friendlycaptcha.eu",
    "smartcaptcha.yandexcloud.net",
    "captcha.qq.com",
    "captcha.qcloud.com",
    "captcha.gtimg.com",
];

/// Captcha-solving services (people or programs paid to solve captchas for a client; review 1 R8: three more, checked
/// in the DNS on 22/11).
pub const SOLVER_HOSTS: [&str; 23] = [
    "2captcha.com",
    "rucaptcha.com",
    "anti-captcha.com",
    "capsolver.com",
    "capmonster.cloud",
    "deathbycaptcha.com",
    "nopecha.com",
    "azcaptcha.com",
    "captchaai.com",
    "nextcaptcha.com",
    "solvecaptcha.com",
    "ez-captcha.com",
    "cap.guru",
    "bestcaptchasolver.com",
    "imagetyperz.com",
    "endcaptcha.com",
    "9kw.eu",
    "truecaptcha.org",
    "anycaptcha.com",
    "captchas.io",
    "nocaptchaai.com",
    "capbypass.com",
    "multibot.in",
];

fn under(host: &str, name: &str) -> bool {
    host == name || host.ends_with(&format!(".{name}"))
}

/// What `host` is (a plain name, `https://…`, or a `*.domain`, which counts when it covers one of them), else None.
pub fn never(host: &str) -> Option<Never> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("https://").or_else(|| host.strip_prefix("http://")).unwrap_or(&host);
    let host = host.split(['/', '?', '#', ':']).next().unwrap_or("");
    let (star, base) = match host.strip_prefix("*.") {
        Some(base) => (true, base),
        None => (false, host),
    };
    if base.is_empty() {
        return None;
    }
    let hit = |name: &&str| under(base, name) || (star && name.ends_with(&format!(".{base}")));
    if SOLVER_HOSTS.iter().any(hit) {
        return Some(Never::Solver);
    }
    if CHECK_HOSTS.iter().any(hit) {
        return Some(Never::Check);
    }
    None
}

/// Why a site is refused for sending, in the agent's words (the tools) and the person's (« Sites autorisés »: the
/// app shows its own text for a 422).
pub fn refusal(host: &str, kind: Never) -> String {
    match kind {
        Never::Check => format!(
            "{host} serves bot checks: never allowed for sending, never reached by your programs. A check is your \
             person's to pass in your browser (« Prendre la main »): don't try again or go round it; tell them in one \
             sentence."
        ),
        Never::Solver => format!(
            "{host} is a captcha-solving service: never used on BipAgents. If a captcha blocks you, tell your person: \
             they can take over your browser (« Prendre la main »)."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_and_solvers_with_their_subdomains() {
        for host in ["challenges.cloudflare.com", "HCAPTCHA.com.", "api.hcaptcha.com", "newassets.hcaptcha.com",
                     "www.recaptcha.net", "geo.captcha-delivery.com", "api-js.datadome.co", "client.px-cloud.net",
                     "client-api.arkoselabs.com", "gcaptcha4.geetest.com", "abc.token.awswaf.com",
                     "https://challenges.cloudflare.com/turnstile/v0/api.js", "collector-pxabc.pxchk.net",
                     "eu-api.friendlycaptcha.eu", "smartcaptcha.yandexcloud.net", "ssl.captcha.qq.com",
                     "turing.captcha.qcloud.com"] {
            assert_eq!(never(host), Some(Never::Check), "{host}");
        }
        for host in ["2captcha.com", "api.2captcha.com", "api.anti-captcha.com", "api.capsolver.com", "api.capmonster.cloud",
                     "rucaptcha.com", "deathbycaptcha.com", "nopecha.com", "api.nocaptchaai.com", "capbypass.com",
                     "multibot.in"] {
            assert_eq!(never(host), Some(Never::Solver), "{host}");
        }
        // a `*.` that covers one of them; ordinary sites and look-alikes are not
        assert_eq!(never("*.cloudflare.com"), Some(Never::Check));
        assert_eq!(never("*.hcaptcha.com"), Some(Never::Check));
        for host in ["cloudflare.com", "www.cloudflare.com", "static.cloudflareinsights.com", "js.datadome.co", "qq.com",
                     "mail.qq.com", "yandexcloud.net", "storage.yandexcloud.net",
                     "datadome.co", "decathlon.fr", "www.google.com", "notrecaptcha.net", "my2captcha.com",
                     "*.decathlon.fr", "", "*."] {
            assert_eq!(never(host), None, "{host}");
        }
    }

    /// The proxy refuses the same hosts: its lists hold these names, in the same kind.
    #[test]
    fn the_proxy_has_the_same_lists() {
        let script = include_str!("../assets/sandbox/egress_proxy.py");
        let list = |name: &str| -> Vec<String> {
            let start = script.find(&format!("{name} = (")).unwrap_or_else(|| panic!("{name} in the proxy"));
            let body = &script[start..start + script[start..].find(')').unwrap()];
            body.split('"').skip(1).step_by(2).map(str::to_string).collect()
        };
        assert_eq!(list("NEVER_CHECKS"), CHECK_HOSTS.to_vec());
        assert_eq!(list("NEVER_SOLVERS"), SOLVER_HOSTS.to_vec());
    }

    #[test]
    fn the_refusals_say_what_to_do() {
        let check = refusal("challenges.cloudflare.com", Never::Check);
        assert!(check.contains("bot checks") && check.contains("« Prendre la main »") && check.contains("go round it"));
        assert!(refusal("2captcha.com", Never::Solver).contains("captcha-solving service"));
    }
}
