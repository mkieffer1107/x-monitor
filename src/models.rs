use std::collections::HashSet;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MonitorKind {
    Account,
    Phrase,
}

impl MonitorKind {
    pub fn display(&self) -> &'static str {
        match self {
            Self::Account => "Account",
            Self::Phrase => "Phrase",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSettings {
    pub enabled: bool,
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub id: Uuid,
    pub label: String,
    pub kind: MonitorKind,
    #[serde(default = "default_monitor_enabled")]
    pub enabled: bool,
    pub input_value: String,
    pub query: String,
    pub rule_id: String,
    pub rule_tag: String,
    pub analysis: AnalysisSettings,
    pub created_at: DateTime<Utc>,
}

fn default_monitor_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonitorStore {
    pub monitors: Vec<Monitor>,
}

#[derive(Debug, Clone)]
pub struct StreamPost {
    pub id: String,
    pub author_id: Option<String>,
    pub author_username: Option<String>,
    pub text: String,
    pub matching_tags: Vec<String>,
}

impl StreamPost {
    pub fn post_url(&self) -> String {
        match &self.author_username {
            Some(username) => format!("https://x.com/{username}/status/{}", self.id),
            None => format!("https://x.com/i/web/status/{}", self.id),
        }
    }
}

#[derive(Debug, Clone)]
pub enum FeedKind {
    Post {
        author: String,
        text: String,
        monitors: Vec<String>,
    },
    Analysis {
        monitor: String,
        provider: String,
        model: String,
        output: String,
    },
    Info(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct FeedItem {
    pub id: Uuid,
    pub at: DateTime<Local>,
    pub kind: FeedKind,
    pub url: Option<String>,
}

impl FeedItem {
    pub fn summary(&self) -> String {
        let ts = self.at.format("%H:%M:%S");
        match &self.kind {
            FeedKind::Post {
                author,
                text,
                monitors,
            } => {
                let label = if monitors.is_empty() {
                    String::from("monitor: unknown")
                } else {
                    format!("monitor: {}", monitors.join(", "))
                };
                format!(
                    "[{ts}] POST @{author} | {label} | {}",
                    text.replace('\n', " ")
                )
            }
            FeedKind::Analysis {
                monitor,
                provider,
                model,
                output,
            } => format!(
                "[{ts}] AI ({provider}:{model}) [{monitor}] {}",
                output.replace('\n', " ")
            ),
            FeedKind::Info(message) => format!("[{ts}] INFO {message}"),
            FeedKind::Error(message) => format!("[{ts}] ERROR {message}"),
        }
    }
}

pub fn build_query(kind: &MonitorKind, target: &str) -> anyhow::Result<String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        anyhow::bail!("target cannot be empty");
    }

    match kind {
        MonitorKind::Account => {
            let handles = parse_account_handles(trimmed)?;
            if handles.len() == 1 {
                Ok(format!("from:{}", handles[0]))
            } else {
                let query = handles
                    .into_iter()
                    .map(|handle| format!("from:{handle}"))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                Ok(format!("({query})"))
            }
        }
        MonitorKind::Phrase => {
            if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
                Ok(trimmed.to_string())
            } else if trimmed.contains(' ') {
                let escaped = trimmed.replace('"', "\\\"");
                Ok(format!("\"{escaped}\""))
            } else {
                Ok(trimmed.to_string())
            }
        }
    }
}

pub fn parse_account_handles(input: &str) -> anyhow::Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut handles = Vec::new();

    for raw_part in input.split(',') {
        let part = raw_part.trim();
        if part.is_empty() {
            continue;
        }

        let normalized = part.trim_start_matches('@').trim();
        if normalized.is_empty() {
            continue;
        }

        if normalized.contains(char::is_whitespace) {
            anyhow::bail!("account handles cannot contain spaces");
        }

        if !normalized
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        {
            anyhow::bail!("account handles may only use letters, numbers, and underscores");
        }

        let key = normalized.to_ascii_lowercase();
        if seen.insert(key) {
            handles.push(normalized.to_string());
        }
    }

    if handles.is_empty() {
        anyhow::bail!("account target requires at least one handle");
    }

    Ok(handles)
}
