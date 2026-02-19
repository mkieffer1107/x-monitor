use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::models::MonitorKind;

#[derive(Debug, Clone)]
pub struct TargetFileMonitor {
    pub label: Option<String>,
    pub kind: MonitorKind,
    pub target: String,
    pub ai_enabled: bool,
    pub ai_provider: Option<String>,
    pub ai_model: Option<String>,
    pub ai_endpoint: Option<String>,
    pub ai_api_key: Option<String>,
    pub ai_prompt: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TargetFileEntry {
    pub file_name: String,
    pub path: PathBuf,
    pub raw: String,
    pub parsed: Result<TargetFileMonitor, String>,
}

#[derive(Debug, Deserialize)]
struct RawTargetFile {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    kind: String,
    target: String,
    #[serde(default)]
    ai: Option<RawAiConfig>,
    #[serde(default)]
    ai_enabled: Option<bool>,
    #[serde(default)]
    ai_provider: Option<String>,
    #[serde(default)]
    ai_model: Option<String>,
    #[serde(default)]
    ai_endpoint: Option<String>,
    #[serde(default)]
    ai_api_key: Option<String>,
    #[serde(default)]
    ai_prompt: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawAiConfig {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

pub fn load_target_file_entries(dir: &Path) -> Result<Vec<TargetFileEntry>> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let mut entries = Vec::new();
    for item in fs::read_dir(dir).with_context(|| format!("failed to list {}", dir.display()))? {
        let item = item?;
        let path = item.path();
        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if !matches!(ext, "yaml" | "yml") {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .unwrap_or_else(|| path.display().to_string());

        match fs::read_to_string(&path) {
            Ok(raw) => {
                let parsed = parse_target_file(&raw).map_err(|error| error.to_string());
                entries.push(TargetFileEntry {
                    file_name,
                    path,
                    raw,
                    parsed,
                });
            }
            Err(error) => {
                entries.push(TargetFileEntry {
                    file_name,
                    path,
                    raw: String::new(),
                    parsed: Err(format!("failed to read file: {error}")),
                });
            }
        }
    }

    entries.sort_by_key(|entry| entry.file_name.to_ascii_lowercase());
    Ok(entries)
}

fn parse_target_file(raw: &str) -> Result<TargetFileMonitor> {
    let parsed: RawTargetFile =
        serde_yaml::from_str(raw).context("invalid YAML format for target config")?;

    let kind = parse_kind(&parsed.kind)?;
    let target = parsed.target.trim().to_string();
    if target.is_empty() {
        anyhow::bail!("target cannot be empty");
    }

    let ai = parsed.ai.unwrap_or_default();
    let ai_provider = clean_opt(parsed.ai_provider.or(ai.provider));
    let ai_model = clean_opt(parsed.ai_model.or(ai.model));
    let ai_endpoint = clean_opt(parsed.ai_endpoint.or(ai.endpoint));
    let ai_api_key = clean_opt(parsed.ai_api_key.or(ai.api_key));
    let ai_prompt = clean_opt(parsed.ai_prompt.or(ai.prompt));
    let any_ai_value = ai_provider.is_some()
        || ai_model.is_some()
        || ai_endpoint.is_some()
        || ai_api_key.is_some()
        || ai_prompt.is_some();
    let ai_enabled = parsed.ai_enabled.or(ai.enabled).unwrap_or(any_ai_value);

    Ok(TargetFileMonitor {
        label: clean_opt(parsed.label.or(parsed.display_name)),
        kind,
        target,
        ai_enabled,
        ai_provider,
        ai_model,
        ai_endpoint,
        ai_api_key,
        ai_prompt,
    })
}

fn parse_kind(kind: &str) -> Result<MonitorKind> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "account" | "accounts" | "acct" => Ok(MonitorKind::Account),
        "phrase" | "phrases" | "keyword" | "keywords" => Ok(MonitorKind::Phrase),
        _ => anyhow::bail!("kind must be 'account' or 'phrase'"),
    }
}

fn clean_opt(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
