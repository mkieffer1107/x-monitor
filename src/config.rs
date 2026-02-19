use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProvider {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
}

impl AiProvider {
    pub fn resolved_api_key(&self) -> Option<String> {
        self.api_key
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .or_else(|| {
                self.api_key_env
                    .as_ref()
                    .and_then(|var| env::var(var).ok())
                    .filter(|value| !value.trim().is_empty())
            })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedAiProvider {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub x_bearer_token: Option<String>,
    #[serde(default = "default_state_path")]
    pub state_path: PathBuf,
    #[serde(default = "default_monitor_config_dir")]
    pub monitor_config_dir: PathBuf,
    #[serde(default = "default_ai_provider_name")]
    pub default_ai_provider: String,
    #[serde(default = "default_ai_providers")]
    pub ai_providers: Vec<AiProvider>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            x_bearer_token: None,
            state_path: default_state_path(),
            monitor_config_dir: default_monitor_config_dir(),
            default_ai_provider: default_ai_provider_name(),
            ai_providers: default_ai_providers(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<(Self, PathBuf, bool)> {
        let config_path = env::var("X_MONITOR_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("x-monitor.toml"));

        let mut created_default = false;
        let mut config = if config_path.exists() {
            let raw = fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            toml::from_str::<Self>(&raw)
                .with_context(|| format!("invalid config at {}", config_path.display()))?
        } else {
            let defaults = Self::default();
            let rendered = toml::to_string_pretty(&defaults)?;
            fs::write(&config_path, rendered)
                .with_context(|| format!("failed to write {}", config_path.display()))?;
            created_default = true;
            defaults
        };

        if let Some(token) = first_non_empty_env(&["X_BEARER_TOKEN", "x_bearer_token"]) {
            config.x_bearer_token = Some(token);
        }

        if let Some(dir) = first_non_empty_env(&["X_MONITOR_CONFIG_DIR", "x_monitor_config_dir"]) {
            config.monitor_config_dir = PathBuf::from(dir);
        }

        if let Some(provider) = first_non_empty_env(&[
            "X_MONITOR_DEFAULT_AI_PROVIDER",
            "x_monitor_default_ai_provider",
        ]) {
            config.default_ai_provider = provider;
        }

        if config.ai_providers.is_empty() {
            config.ai_providers = default_ai_providers();
        } else {
            merge_default_providers(&mut config.ai_providers);
        }

        if !config.ai_providers.iter().any(|provider| {
            provider
                .name
                .eq_ignore_ascii_case(&config.default_ai_provider)
        }) {
            config.default_ai_provider = config
                .ai_providers
                .first()
                .map(|provider| provider.name.clone())
                .unwrap_or_else(default_ai_provider_name);
        }

        Ok((config, config_path, created_default))
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.ai_providers
            .iter()
            .map(|provider| provider.name.clone())
            .collect()
    }

    pub fn provider_by_name(&self, name: &str) -> Option<&AiProvider> {
        self.ai_providers
            .iter()
            .find(|provider| provider.name.eq_ignore_ascii_case(name))
    }

    pub fn resolve_provider(&self, name: &str) -> Option<ResolvedAiProvider> {
        self.provider_by_name(name).and_then(|provider| {
            provider
                .resolved_api_key()
                .map(|api_key| ResolvedAiProvider {
                    name: provider.name.clone(),
                    base_url: provider.base_url.clone(),
                    model: provider.model.clone(),
                    api_key,
                })
        })
    }
}

fn first_non_empty_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn default_state_path() -> PathBuf {
    PathBuf::from("x-monitor-state.json")
}

fn default_monitor_config_dir() -> PathBuf {
    PathBuf::from("monitor-configs")
}

fn default_ai_provider_name() -> String {
    "grok".to_string()
}

fn default_ai_providers() -> Vec<AiProvider> {
    vec![
        AiProvider {
            name: "grok".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "grok-4-1-fast-non-reasoning".to_string(),
            api_key: None,
            api_key_env: Some("XAI_API_KEY".to_string()),
        },
        AiProvider {
            name: "openrouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            model: "x-ai/grok-4.1-fast".to_string(),
            api_key: None,
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
        },
        AiProvider {
            name: "gemini".to_string(),
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
            model: "gemini-3-flash-preview".to_string(),
            api_key: None,
            api_key_env: Some("GEMINI_API_KEY".to_string()),
        },
        AiProvider {
            name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5-nano".to_string(),
            api_key: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        },
        AiProvider {
            name: "custom".to_string(),
            base_url: String::new(),
            model: String::new(),
            api_key: None,
            api_key_env: None,
        },
    ]
}

fn merge_default_providers(existing: &mut Vec<AiProvider>) {
    let mut remaining = std::mem::take(existing);
    let mut merged = Vec::new();

    for default in default_ai_providers() {
        if let Some(position) = remaining
            .iter()
            .position(|provider| provider.name.eq_ignore_ascii_case(&default.name))
        {
            merged.push(remaining.remove(position));
        } else {
            merged.push(default);
        }
    }

    merged.extend(remaining);
    *existing = merged;
}
