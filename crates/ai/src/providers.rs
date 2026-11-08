//! Providers we know by name: where they answer, which environment variable holds the key, and the
//! model a new backend starts on. One table, so detection, key lookup and the UI agree.

use crate::BackendKind;

/// One hosted provider, as a row of data rather than a match arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    /// The name a backend is added under.
    pub name: &'static str,
    pub kind: BackendKind,
    /// The OpenAI-compatible base URL, or empty for a native API.
    pub base_url: &'static str,
    /// The environment variable that conventionally holds its key.
    pub key_env: &'static str,
    /// A second name some tools use for the same key.
    pub alt_key_env: Option<&'static str>,
    /// The model a new backend starts on, before the provider's own list is fetched.
    pub default_model: &'static str,
    /// Its id in the shared models.dev catalogue, where it differs from ours.
    pub catalog_id: &'static str,
}

/// The priced APIs we can talk to over an OpenAI-compatible endpoint. Pay-as-you-go services are
/// offered only when their key is already set, like OpenAI and Anthropic, so nobody is advertised
/// a service they cannot use.
pub const PROVIDERS: &[Provider] = &[
    Provider {
        name: "Moonshot",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.moonshot.ai/v1",
        key_env: "MOONSHOT_API_KEY",
        alt_key_env: None,
        default_model: "kimi-k3.1-pro",
        catalog_id: "moonshotai",
    },
    Provider {
        name: "DeepSeek",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.deepseek.com/v1",
        key_env: "DEEPSEEK_API_KEY",
        alt_key_env: None,
        default_model: "deepseek-flash",
        catalog_id: "deepseek",
    },
    Provider {
        name: "xAI (Grok)",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.x.ai/v1",
        key_env: "XAI_API_KEY",
        alt_key_env: None,
        default_model: "grok-4.7",
        catalog_id: "xai",
    },
    Provider {
        name: "Groq",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.groq.com/openai/v1",
        key_env: "GROQ_API_KEY",
        alt_key_env: None,
        default_model: "openai/gpt-oss-120b",
        catalog_id: "groq",
    },
    Provider {
        name: "Mistral",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.mistral.ai/v1",
        key_env: "MISTRAL_API_KEY",
        alt_key_env: None,
        default_model: "mistral-large-4",
        catalog_id: "mistral",
    },
    Provider {
        name: "Together",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.together.xyz/v1",
        key_env: "TOGETHER_API_KEY",
        alt_key_env: None,
        default_model: "deepseek-ai/DeepSeek-V4.1-Flash",
        catalog_id: "togetherai",
    },
    Provider {
        name: "Fireworks",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.fireworks.ai/inference/v1",
        key_env: "FIREWORKS_API_KEY",
        alt_key_env: None,
        default_model: "accounts/fireworks/models/gpt-oss-120b",
        catalog_id: "fireworks-ai",
    },
    Provider {
        name: "DeepInfra",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.deepinfra.com/v1/openai",
        key_env: "DEEPINFRA_API_KEY",
        alt_key_env: None,
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        catalog_id: "deepinfra",
    },
    Provider {
        name: "GLM (Z.ai)",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.z.ai/api/paas/v4",
        key_env: "ZAI_API_KEY",
        alt_key_env: None,
        default_model: "glm-5.3",
        catalog_id: "z-ai",
    },
    Provider {
        name: "MiniMax",
        kind: BackendKind::OpenaiCompatible,
        base_url: "https://api.minimax.io/v1",
        key_env: "MINIMAX_API_KEY",
        alt_key_env: None,
        default_model: "MiniMax-M3.1-Flash",
        catalog_id: "minimax",
    },
];

/// One provider offered in the "Add a provider" picker: the hosted APIs a person can connect with
/// an API key, native Anthropic and OpenAI kinds included alongside OpenAI-compatible services.
/// A row of data, not a match arm, so the dropdown, detection and key lookup agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderChoice {
    /// The name a backend is added under.
    pub name: &'static str,
    pub kind: BackendKind,
    /// The OpenAI-compatible base URL, or `None` for a native API (its own default base is used).
    pub base_url: Option<&'static str>,
    /// The environment variable that conventionally holds its key.
    pub key_env: &'static str,
    /// The model a new backend starts on, before the provider's own list is fetched.
    pub default_model: &'static str,
    /// The models currently offered, for the picker when the provider's live list can't be fetched
    /// (for example GLM (Z.ai), which has no OpenAI `/models` endpoint).
    pub models: &'static [&'static str],
}

/// The providers offered in Settings → AI's dropdown, in the order they're shown.
pub const PROVIDER_CHOICES: &[ProviderChoice] = &[
    ProviderChoice {
        name: "Kimi (Moonshot)",
        kind: BackendKind::OpenaiCompatible,
        base_url: Some("https://api.moonshot.ai/v1"),
        key_env: "MOONSHOT_API_KEY",
        default_model: "kimi-k3.1-pro",
        models: &["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed"],
    },
    ProviderChoice {
        name: "DeepSeek",
        kind: BackendKind::OpenaiCompatible,
        base_url: Some("https://api.deepseek.com/v1"),
        key_env: "DEEPSEEK_API_KEY",
        default_model: "deepseek-flash",
        models: &["deepseek-flash", "deepseek-v4-pro"],
    },
    ProviderChoice {
        name: "Claude (Anthropic)",
        kind: BackendKind::Anthropic,
        base_url: None,
        key_env: "ANTHROPIC_API_KEY",
        default_model: crate::anthropic::DEFAULT_MODEL,
        models: crate::anthropic::MODELS,
    },
    ProviderChoice {
        name: "ChatGPT (OpenAI)",
        kind: BackendKind::Openai,
        base_url: None,
        key_env: "OPENAI_API_KEY",
        default_model: crate::openai::OPENAI_DEFAULT_MODEL,
        models: &["gpt-5"],
    },
    ProviderChoice {
        name: "GLM (Z.ai)",
        kind: BackendKind::OpenaiCompatible,
        base_url: Some("https://api.z.ai/api/paas/v4"),
        key_env: "ZAI_API_KEY",
        default_model: "glm-5.3",
        models: &["glm-5.3", "glm-5.3-flash", "glm-5.3-flashx", "glm-5.2", "glm-5.1"],
    },
    ProviderChoice {
        name: "MiniMax",
        kind: BackendKind::OpenaiCompatible,
        base_url: Some("https://api.minimax.io/v1"),
        key_env: "MINIMAX_API_KEY",
        default_model: "MiniMax-M3.1-Flash",
        models: &["MiniMax-M3.1-Flash", "MiniMax-M3"],
    },
];

impl ProviderChoice {
    /// The choice a configured backend matches: by kind and (for OpenAI-compatible services) its
    /// base URL, so a backend added by detection ("Moonshot", "Claude") finds the same row as the
    /// picker's ("Kimi (Moonshot)", "Claude (Anthropic)").
    pub fn matching(kind: BackendKind, base: Option<&str>) -> Option<&'static ProviderChoice> {
        PROVIDER_CHOICES.iter().find(|c| {
            c.kind == kind
                && match (c.base_url, base) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.trim_end_matches('/') == b.trim_end_matches('/'),
                    _ => false,
                }
        })
    }
}

impl Provider {
    /// The key names this provider accepts, in order.
    pub fn key_names(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.key_env).chain(self.alt_key_env)
    }
}

/// Split a base URL into scheme, host and path, lowercasing the host.
fn split(base: &str) -> Option<(&str, String, &str)> {
    let (scheme, rest) = base.trim().split_once("://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    Some((scheme, host.to_ascii_lowercase(), path))
}

/// The provider whose base URL this is, if any.
///
/// HTTPS only: a key must never be handed to a plain-HTTP base URL that a settings file could be
/// edited into. A pasted base without the trailing `/v1` still finds its row.
pub fn of_base(base: &str) -> Option<&'static Provider> {
    let (scheme, host, path) = split(base.trim().trim_end_matches('/'))?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let path = path.trim_end_matches('/');
    PROVIDERS.iter().find(|p| {
        let Some((_, _, theirs)) = split(p.base_url) else { return false };
        let Some((_, name, _)) = split(p.base_url) else { return false };
        let theirs = theirs.trim_matches('/');
        name == host && (path == theirs || path.is_empty() || format!("{theirs}/v1") == path || format!("{path}/v1") == theirs)
    })
}

/// The key this provider holds in the environment, when one is set. Blank counts as unset.
pub fn env_key(base: &str) -> Option<String> {
    env_key_with(base, |name| std::env::var(name).ok())
}

/// The same, with the environment passed in, so tests need no process state.
pub fn env_key_with(base: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let provider = of_base(base)?;
    provider.key_names().filter_map(get).find(|k| !k.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row has what a backend needs, and the names are unique.
    #[test]
    fn every_provider_is_complete_and_named_once() {
        for p in PROVIDERS {
            assert!(!p.name.trim().is_empty());
            assert!(!p.key_env.trim().is_empty(), "{} has no key variable", p.name);
            assert!(!p.default_model.trim().is_empty(), "{} has no default model", p.name);
            assert!(!p.catalog_id.trim().is_empty(), "{} has no catalogue id", p.name);
            if p.kind == BackendKind::OpenaiCompatible {
                assert!(p.base_url.starts_with("https://"), "{} is not https", p.name);
            }
        }
        let mut names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PROVIDERS.len(), "two providers share a name");
    }

    /// Two rows must never share a key or a base: one key would be sent to two services.
    #[test]
    fn keys_and_bases_are_unique() {
        let mut keys: Vec<&str> = PROVIDERS.iter().map(|p| p.key_env).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), PROVIDERS.len(), "two providers share a key variable");

        let mut bases: Vec<&str> = PROVIDERS.iter().map(|p| p.base_url).collect();
        bases.sort_unstable();
        bases.dedup();
        assert_eq!(bases.len(), PROVIDERS.len(), "two providers share a base URL");

        let mut catalog: Vec<&str> = PROVIDERS.iter().map(|p| p.catalog_id).collect();
        catalog.sort_unstable();
        catalog.dedup();
        assert_eq!(catalog.len(), PROVIDERS.len(), "two providers share a catalogue id");
    }

    /// Every dropdown choice has what an add needs, names and keys are unique, and the OpenAI-compatible
    /// ones also appear in [`PROVIDERS`] so detection and key lookup stay in step.
    #[test]
    fn every_provider_choice_is_complete_and_unique() {
        let mut names: Vec<&str> = PROVIDER_CHOICES.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PROVIDER_CHOICES.len(), "two choices share a name");
        let mut keys: Vec<&str> = PROVIDER_CHOICES.iter().map(|c| c.key_env).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), PROVIDER_CHOICES.len(), "two choices share a key variable");
        for c in PROVIDER_CHOICES {
            assert!(!c.name.trim().is_empty());
            assert!(!c.key_env.trim().is_empty(), "{} has no key variable", c.name);
            assert!(!c.default_model.trim().is_empty(), "{} has no default model", c.name);
            assert!(!c.models.is_empty(), "{} has no models", c.name);
            assert!(c.models.contains(&c.default_model), "{}'s default model isn't in its list", c.name);
            if c.kind == BackendKind::OpenaiCompatible {
                let base = c.base_url.expect("an OpenAI-compatible choice has a base URL");
                assert!(base.starts_with("https://"), "{} is not https", c.name);
                assert!(PROVIDERS.iter().any(|p| p.base_url == base), "{} isn't in PROVIDERS", c.name);
            } else {
                assert!(c.base_url.is_none(), "a native {} has no base URL", c.name);
            }
        }
        // The exact six the picker offers.
        assert_eq!(
            PROVIDER_CHOICES.iter().map(|c| c.name).collect::<Vec<_>>(),
            ["Kimi (Moonshot)", "DeepSeek", "Claude (Anthropic)", "ChatGPT (OpenAI)", "GLM (Z.ai)", "MiniMax"]
        );
    }

    /// A backend configured by detection ("Moonshot", "Claude") finds the same picker row as the
    /// dropdown's names ("Kimi (Moonshot)", "Claude (Anthropic)"), by kind and base URL.
    #[test]
    fn a_backend_matches_its_provider_choice_by_kind_and_base() {
        assert_eq!(ProviderChoice::matching(BackendKind::OpenaiCompatible, Some("https://api.moonshot.ai/v1")).unwrap().name, "Kimi (Moonshot)");
        assert_eq!(ProviderChoice::matching(BackendKind::OpenaiCompatible, Some("https://api.deepseek.com/v1/")).unwrap().name, "DeepSeek");
        assert_eq!(ProviderChoice::matching(BackendKind::Anthropic, None).unwrap().name, "Claude (Anthropic)");
        assert_eq!(ProviderChoice::matching(BackendKind::Openai, None).unwrap().name, "ChatGPT (OpenAI)");
        assert_eq!(ProviderChoice::matching(BackendKind::OpenaiCompatible, Some("https://api.z.ai/api/paas/v4")).unwrap().name, "GLM (Z.ai)");
        assert_eq!(ProviderChoice::matching(BackendKind::OpenaiCompatible, Some("https://api.minimax.io/v1")).unwrap().name, "MiniMax");
        assert!(ProviderChoice::matching(BackendKind::OpenaiCompatible, Some("http://127.0.0.1:11434/v1")).is_none());
        assert!(ProviderChoice::matching(BackendKind::Anthropic, Some("https://example.com")).is_none());
    }

    /// A plain-HTTP base never receives a key, and a base missing `/v1` still finds its row.
    #[test]
    fn only_https_bases_are_recognised() {
        assert!(of_base("http://api.deepseek.com/v1").is_none());
        assert_eq!(of_base("https://api.deepseek.com"), Some(&PROVIDERS[1]));
        assert_eq!(of_base("HTTPS://API.DEEPSEEK.COM/v1"), Some(&PROVIDERS[1]));
    }

    /// A blank variable counts as unset, so the next name is tried.
    #[test]
    fn a_blank_key_falls_through_to_the_next_name() {
        let base = "https://api.deepseek.com/v1";
        let empty = env_key_with(base, |_| Some("  ".into()));
        assert_eq!(empty, None);
        let found = env_key_with(base, |name| (name == "DEEPSEEK_API_KEY").then(|| "k".to_string()));
        assert_eq!(found.as_deref(), Some("k"));
    }

    /// A base URL finds its row, with or without a trailing slash; someone else's doesn't.
    #[test]
    fn a_base_url_finds_its_provider() {
        assert_eq!(of_base("https://api.deepseek.com/v1"), Some(&PROVIDERS[1]));
        assert_eq!(of_base("https://api.deepseek.com/v1/"), Some(&PROVIDERS[1]));
        assert_eq!(of_base("http://127.0.0.1:11434/v1"), None);
        assert_eq!(of_base("https://api.example.test/v1"), None);
    }
}
