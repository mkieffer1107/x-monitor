use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use chrono::Local;
use serde_json::to_string_pretty;
use uuid::Uuid;

use crate::{
    config::AppConfig,
    models::{
        AnalysisSettings, FeedItem, FeedKind, Monitor, MonitorKind, MonitorStore, StreamPost,
        build_query, parse_account_handles,
    },
    target_files::{TargetFileEntry, load_target_file_entries},
};

const MAX_FEED_ITEMS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Monitors,
    Feed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorFormMode {
    Add,
    Edit,
}

#[derive(Debug, Clone)]
pub struct AddMonitorForm {
    pub mode: MonitorFormMode,
    pub field_index: usize,
    pub kind: MonitorKind,
    pub target: String,
    pub display_name: String,
    pub ai_enabled: bool,
    pub ai_provider_index: usize,
    pub ai_model: String,
    pub ai_endpoint: String,
    pub ai_api_key: String,
    pub ai_prompt: String,
}

impl AddMonitorForm {
    pub fn new(config: &AppConfig, provider_names: &[String], default_provider: &str) -> Self {
        let ai_provider_index = provider_names
            .iter()
            .position(|name| name == default_provider)
            .unwrap_or(0);

        let mut form = Self {
            mode: MonitorFormMode::Add,
            field_index: 0,
            kind: MonitorKind::Account,
            target: String::new(),
            display_name: String::new(),
            ai_enabled: false,
            ai_provider_index,
            ai_model: String::new(),
            ai_endpoint: String::new(),
            ai_api_key: String::new(),
            ai_prompt: "Summarize why this post matters and what to watch next.".to_string(),
        };
        form.apply_provider_defaults(config, provider_names);
        form
    }

    pub fn from_monitor(config: &AppConfig, provider_names: &[String], monitor: &Monitor) -> Self {
        let default_index = provider_names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&config.default_ai_provider))
            .unwrap_or(0);
        let ai_provider_index = provider_names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&monitor.analysis.provider))
            .unwrap_or(default_index);

        Self {
            mode: MonitorFormMode::Edit,
            field_index: 0,
            kind: monitor.kind.clone(),
            target: monitor.input_value.clone(),
            display_name: monitor.label.clone(),
            ai_enabled: monitor.analysis.enabled,
            ai_provider_index,
            ai_model: monitor.analysis.model.clone(),
            ai_endpoint: monitor.analysis.endpoint.clone(),
            ai_api_key: monitor.analysis.api_key.clone(),
            ai_prompt: monitor.analysis.prompt.clone(),
        }
    }

    pub fn selected_provider(&self, provider_names: &[String]) -> String {
        provider_names
            .get(self.ai_provider_index)
            .cloned()
            .or_else(|| provider_names.first().cloned())
            .unwrap_or_else(|| "grok".to_string())
    }

    pub fn cycle_provider(&mut self, provider_names: &[String], delta: i32) {
        if provider_names.is_empty() {
            self.ai_provider_index = 0;
            return;
        }

        let len = provider_names.len() as i32;
        let next = (self.ai_provider_index as i32 + delta).rem_euclid(len);
        self.ai_provider_index = next as usize;
    }

    pub fn apply_provider_defaults(&mut self, config: &AppConfig, provider_names: &[String]) {
        let provider_name = self.selected_provider(provider_names);
        if let Some(provider) = config.provider_by_name(&provider_name) {
            self.ai_model = provider.model.clone();
            self.ai_endpoint = provider.base_url.clone();
            self.ai_api_key = if provider.name.eq_ignore_ascii_case("custom") {
                String::new()
            } else {
                provider
                    .api_key_env
                    .clone()
                    .or_else(|| provider.api_key.clone())
                    .unwrap_or_default()
            };
        }
    }

    pub fn cycle_kind(&mut self, delta: i32) {
        self.kind = match (self.kind.clone(), delta.signum()) {
            (MonitorKind::Account, d) if d >= 0 => MonitorKind::Phrase,
            (MonitorKind::Phrase, d) if d >= 0 => MonitorKind::Account,
            (MonitorKind::Account, _) => MonitorKind::Phrase,
            (MonitorKind::Phrase, _) => MonitorKind::Account,
        };
    }

    pub fn move_field(&mut self, delta: i32) {
        let count = 10i32;
        let next = (self.field_index as i32 + delta).rem_euclid(count);
        self.field_index = next as usize;
    }

    pub fn to_pending_monitor(&self, provider_names: &[String]) -> Result<PendingMonitor> {
        let id = Uuid::new_v4();
        let query = build_query(&self.kind, &self.target)?;

        let (input_value, default_label) = match self.kind {
            MonitorKind::Account => {
                let handles = parse_account_handles(&self.target)?;
                let target = handles.join(", ");
                let label = handles
                    .iter()
                    .map(|handle| format!("@{handle}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (target, label)
            }
            MonitorKind::Phrase => {
                let trimmed = self.target.trim().to_string();
                (trimmed.clone(), trimmed)
            }
        };

        let label = if self.display_name.trim().is_empty() {
            default_label
        } else {
            self.display_name.trim().to_string()
        };

        if label.is_empty() {
            anyhow::bail!("display name cannot be empty");
        }

        let analysis = AnalysisSettings {
            enabled: self.ai_enabled,
            provider: self.selected_provider(provider_names),
            model: self.ai_model.trim().to_string(),
            endpoint: self.ai_endpoint.trim().to_string(),
            api_key: self.ai_api_key.trim().to_string(),
            prompt: self.ai_prompt.trim().to_string(),
        };

        if analysis.enabled {
            if analysis.model.is_empty() {
                anyhow::bail!("AI model ID cannot be empty when analysis is enabled");
            }

            if analysis.provider.eq_ignore_ascii_case("custom") && analysis.endpoint.is_empty() {
                anyhow::bail!("custom AI provider requires an endpoint");
            }
            if analysis.provider.eq_ignore_ascii_case("custom") && analysis.api_key.is_empty() {
                anyhow::bail!("custom AI provider requires an API key");
            }
        }

        Ok(PendingMonitor {
            id,
            label,
            kind: self.kind.clone(),
            enabled: true,
            input_value,
            query,
            rule_tag: format!("xmon:{}", id.simple()),
            analysis,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PendingMonitor {
    pub id: Uuid,
    pub label: String,
    pub kind: MonitorKind,
    pub enabled: bool,
    pub input_value: String,
    pub query: String,
    pub rule_tag: String,
    pub analysis: AnalysisSettings,
}

#[derive(Debug, Clone)]
pub struct EditSession {
    pub original_monitor: Monitor,
}

#[derive(Debug, Clone)]
pub struct TargetFilePicker {
    pub directory: PathBuf,
    pub entries: Vec<TargetFileEntry>,
    pub selected: usize,
}

#[derive(Debug)]
pub struct App {
    pub should_quit: bool,
    pub focus: FocusPane,
    pub monitors: Vec<Monitor>,
    pub selected_monitor: usize,
    pub feed: VecDeque<FeedItem>,
    pub selected_feed: usize,
    pub add_form: Option<AddMonitorForm>,
    pub edit_session: Option<EditSession>,
    pub target_file_picker: Option<TargetFilePicker>,
    pub status: String,
    pub provider_names: Vec<String>,
    stream_connected: bool,
    monitor_activity: HashMap<Uuid, bool>,
    monitor_initiating: HashSet<Uuid>,
    state_path: PathBuf,
    pub config: AppConfig,
}

impl App {
    pub fn new(config: AppConfig, state_path: PathBuf, monitors: Vec<Monitor>) -> Self {
        let monitor_activity = monitors
            .iter()
            .map(|monitor| (monitor.id, false))
            .collect::<HashMap<_, _>>();

        Self {
            should_quit: false,
            focus: FocusPane::Monitors,
            monitors,
            selected_monitor: 0,
            feed: VecDeque::new(),
            selected_feed: 0,
            add_form: None,
            edit_session: None,
            target_file_picker: None,
            status: "Ready".to_string(),
            provider_names: config.provider_names(),
            stream_connected: false,
            monitor_activity,
            monitor_initiating: HashSet::new(),
            state_path,
            config,
        }
    }

    pub fn load_store(path: &PathBuf) -> Result<Vec<Monitor>> {
        if !path.exists() {
            return Ok(Vec::new());
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read monitor state at {}", path.display()))?;
        let store: MonitorStore = serde_json::from_str(&raw)
            .with_context(|| format!("invalid monitor state at {}", path.display()))?;
        Ok(store.monitors)
    }

    pub fn save_store(&self) -> Result<()> {
        let state = MonitorStore {
            monitors: self.monitors.clone(),
        };

        let body = to_string_pretty(&state).context("failed to serialize monitor state")?;
        fs::write(&self.state_path, body)
            .with_context(|| format!("failed to write {}", self.state_path.display()))?;
        Ok(())
    }

    pub fn push_feed(&mut self, item: FeedItem) {
        self.feed.push_front(item);
        while self.feed.len() > MAX_FEED_ITEMS {
            self.feed.pop_back();
        }

        if self.selected_feed >= self.feed.len() {
            self.selected_feed = self.feed.len().saturating_sub(1);
        }
    }

    pub fn clear_feed(&mut self) {
        self.feed.clear();
        self.selected_feed = 0;
    }

    pub fn push_info(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.push_feed(FeedItem {
            id: Uuid::new_v4(),
            at: Local::now(),
            kind: FeedKind::Info(message),
            url: None,
        });
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.push_feed(FeedItem {
            id: Uuid::new_v4(),
            at: Local::now(),
            kind: FeedKind::Error(message),
            url: None,
        });
    }

    pub fn push_post(&mut self, post: &StreamPost, monitors: Vec<String>) {
        let author = post
            .author_username
            .clone()
            .or(post.author_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        self.push_feed(FeedItem {
            id: Uuid::new_v4(),
            at: Local::now(),
            kind: FeedKind::Post {
                author,
                text: post.text.clone(),
                monitors,
            },
            url: Some(post.post_url()),
        });
    }

    pub fn push_analysis(
        &mut self,
        monitor: String,
        provider: String,
        model: String,
        output: String,
        url: Option<String>,
    ) {
        self.push_feed(FeedItem {
            id: Uuid::new_v4(),
            at: Local::now(),
            kind: FeedKind::Analysis {
                monitor,
                provider,
                model,
                output,
            },
            url,
        });
    }

    pub fn monitor_by_tag(&self, tag: &str) -> Option<&Monitor> {
        self.monitors.iter().find(|monitor| monitor.rule_tag == tag)
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Monitors => FocusPane::Feed,
            FocusPane::Feed => FocusPane::Monitors,
        }
    }

    pub fn move_selection_up(&mut self) {
        match self.focus {
            FocusPane::Monitors => {
                if !self.monitors.is_empty() {
                    self.selected_monitor = self
                        .selected_monitor
                        .saturating_sub(1)
                        .min(self.monitors.len().saturating_sub(1));
                }
            }
            FocusPane::Feed => {
                if !self.feed.is_empty() {
                    self.selected_feed = self.selected_feed.saturating_sub(1);
                }
            }
        }
    }

    pub fn move_selection_down(&mut self) {
        match self.focus {
            FocusPane::Monitors => {
                if !self.monitors.is_empty() {
                    self.selected_monitor =
                        (self.selected_monitor + 1).min(self.monitors.len().saturating_sub(1));
                }
            }
            FocusPane::Feed => {
                if !self.feed.is_empty() {
                    self.selected_feed =
                        (self.selected_feed + 1).min(self.feed.len().saturating_sub(1));
                }
            }
        }
    }

    pub fn open_add_form(&mut self) {
        self.add_form = Some(AddMonitorForm::new(
            &self.config,
            &self.provider_names,
            &self.config.default_ai_provider,
        ));
        self.edit_session = None;
        self.target_file_picker = None;
    }

    pub fn open_edit_form(&mut self, monitor: Monitor) {
        self.add_form = Some(AddMonitorForm::from_monitor(
            &self.config,
            &self.provider_names,
            &monitor,
        ));
        self.edit_session = Some(EditSession {
            original_monitor: monitor,
        });
        self.target_file_picker = None;
    }

    pub fn close_add_form(&mut self) {
        self.add_form = None;
        self.edit_session = None;
        self.target_file_picker = None;
    }

    pub fn open_target_file_picker(&mut self) -> Result<usize> {
        let directory = self.config.monitor_config_dir.clone();
        let entries = load_target_file_entries(&directory)?;
        let count = entries.len();
        self.target_file_picker = Some(TargetFilePicker {
            directory,
            entries,
            selected: 0,
        });
        Ok(count)
    }

    pub fn close_target_file_picker(&mut self) {
        self.target_file_picker = None;
    }

    pub fn move_target_file_selection(&mut self, delta: i32) {
        let Some(picker) = self.target_file_picker.as_mut() else {
            return;
        };
        if picker.entries.is_empty() {
            picker.selected = 0;
            return;
        }

        let len = picker.entries.len() as i32;
        let next = (picker.selected as i32 + delta).rem_euclid(len);
        picker.selected = next as usize;
    }

    pub fn selected_target_file_entry(&self) -> Option<&TargetFileEntry> {
        let picker = self.target_file_picker.as_ref()?;
        picker.entries.get(picker.selected)
    }

    pub fn selected_monitor(&self) -> Option<&Monitor> {
        self.monitors.get(self.selected_monitor)
    }

    pub fn selected_feed_item(&self) -> Option<&FeedItem> {
        self.feed.get(self.selected_feed)
    }

    pub fn add_monitor(&mut self, monitor: Monitor) {
        self.monitor_activity
            .insert(monitor.id, monitor.enabled && self.stream_connected);
        self.monitors.push(monitor);
        self.selected_monitor = self.monitors.len().saturating_sub(1);
    }

    pub fn replace_monitor(&mut self, monitor: Monitor) -> bool {
        if let Some(existing) = self
            .monitors
            .iter_mut()
            .find(|existing| existing.id == monitor.id)
        {
            *existing = monitor.clone();
            self.monitor_activity
                .insert(monitor.id, monitor.enabled && self.stream_connected);
            return true;
        }
        false
    }

    pub fn remove_monitor_by_id(&mut self, monitor_id: Uuid) -> Option<Monitor> {
        let position = self
            .monitors
            .iter()
            .position(|monitor| monitor.id == monitor_id)?;
        let removed = self.monitors.remove(position);
        self.monitor_activity.remove(&removed.id);
        self.monitor_initiating.remove(&removed.id);

        if self.selected_monitor >= self.monitors.len() && !self.monitors.is_empty() {
            self.selected_monitor = self.monitors.len() - 1;
        } else if self.monitors.is_empty() {
            self.selected_monitor = 0;
        }

        Some(removed)
    }

    pub fn set_all_monitors_active(&mut self, active: bool) {
        for monitor in &self.monitors {
            self.monitor_activity
                .insert(monitor.id, monitor.enabled && active);
        }
    }

    pub fn set_stream_connected(&mut self, connected: bool) {
        self.stream_connected = connected;
        self.set_all_monitors_active(connected);
        if connected {
            self.monitor_initiating.clear();
        }
    }

    pub fn stream_connected(&self) -> bool {
        self.stream_connected
    }

    pub fn set_monitor_active(&mut self, monitor_id: Uuid, active: bool) {
        self.monitor_activity.insert(monitor_id, active);
    }

    pub fn set_monitor_initiating(&mut self, monitor_id: Uuid, initiating: bool) {
        if initiating {
            self.monitor_initiating.insert(monitor_id);
        } else {
            self.monitor_initiating.remove(&monitor_id);
        }
    }

    pub fn set_enabled_monitors_initiating(&mut self) {
        for monitor in &self.monitors {
            if monitor.enabled {
                self.monitor_initiating.insert(monitor.id);
            } else {
                self.monitor_initiating.remove(&monitor.id);
            }
        }
    }

    pub fn has_initiating_monitors(&self) -> bool {
        !self.monitor_initiating.is_empty()
    }

    pub fn monitor_is_initiating(&self, monitor_id: Uuid) -> bool {
        self.monitor_initiating.contains(&monitor_id)
    }

    pub fn monitor_is_active(&self, monitor_id: Uuid) -> bool {
        self.monitor_activity
            .get(&monitor_id)
            .copied()
            .unwrap_or(false)
    }

    pub fn refresh_monitor_connection_state(&mut self) {
        self.set_all_monitors_active(self.stream_connected);
        if self.stream_connected {
            self.monitor_initiating.clear();
            return;
        }

        self.monitor_initiating.retain(|monitor_id| {
            self.monitors
                .iter()
                .any(|m| m.id == *monitor_id && m.enabled)
        });
    }

    pub fn update_monitor_rule_id(&mut self, monitor_id: Uuid, new_rule_id: String) -> bool {
        self.activate_monitor_with_rule(monitor_id, new_rule_id)
    }

    pub fn activate_monitor_with_rule(&mut self, monitor_id: Uuid, new_rule_id: String) -> bool {
        if let Some(monitor) = self
            .monitors
            .iter_mut()
            .find(|monitor| monitor.id == monitor_id)
        {
            monitor.rule_id = new_rule_id;
            monitor.enabled = true;
            self.monitor_activity
                .insert(monitor_id, self.stream_connected);
            if self.stream_connected {
                self.monitor_initiating.remove(&monitor_id);
            }
            return true;
        }

        false
    }

    pub fn deactivate_monitor(&mut self, monitor_id: Uuid) -> bool {
        if let Some(monitor) = self
            .monitors
            .iter_mut()
            .find(|monitor| monitor.id == monitor_id)
        {
            monitor.enabled = false;
            monitor.rule_id.clear();
            self.monitor_activity.insert(monitor_id, false);
            self.monitor_initiating.remove(&monitor_id);
            return true;
        }
        false
    }

    pub fn disable_monitor_preserve_rule(&mut self, monitor_id: Uuid) -> bool {
        if let Some(monitor) = self
            .monitors
            .iter_mut()
            .find(|monitor| monitor.id == monitor_id)
        {
            monitor.enabled = false;
            self.monitor_activity.insert(monitor_id, false);
            self.monitor_initiating.remove(&monitor_id);
            return true;
        }
        false
    }

    pub fn has_enabled_monitors(&self) -> bool {
        self.monitors.iter().any(|monitor| monitor.enabled)
    }
}
