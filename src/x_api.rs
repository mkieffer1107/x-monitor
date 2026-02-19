use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result};
use futures_util::TryStreamExt;
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{mpsc::UnboundedSender, watch},
    time::sleep,
};
use tokio_util::io::StreamReader;

use crate::{AppMsg, models::StreamPost};

const API_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const STREAM_TCP_KEEPALIVE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct XApiClient {
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct RuleData {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamRule {
    pub id: String,
    pub tag: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddRuleResponse {
    data: Option<Vec<RuleData>>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Debug, Deserialize)]
struct GetRulesResponse {
    data: Option<Vec<StreamRule>>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    detail: Option<String>,
    title: Option<String>,
    value: Option<String>,
}

#[derive(Debug, Serialize)]
struct AddRuleBody {
    add: Vec<AddRule>,
}

#[derive(Debug, Serialize)]
struct AddRule {
    value: String,
    tag: String,
}

#[derive(Debug, Serialize)]
struct DeleteRuleBody {
    delete: DeleteRule,
}

#[derive(Debug, Serialize)]
struct DeleteRule {
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TerminateConnectionsResponse {
    data: Option<TerminateConnectionsData>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Debug, Deserialize)]
struct TerminateConnectionsData {
    killed_connections: Option<bool>,
    successful_kills: Option<u64>,
    failed_kills: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct StreamEnvelope {
    data: Option<StreamData>,
    includes: Option<StreamIncludes>,
    matching_rules: Option<Vec<MatchingRule>>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Debug, Deserialize)]
struct StreamData {
    id: String,
    text: String,
    author_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamIncludes {
    users: Option<Vec<StreamUser>>,
}

#[derive(Debug, Deserialize)]
struct StreamUser {
    id: String,
    username: String,
}

#[derive(Debug, Deserialize)]
struct MatchingRule {
    tag: Option<String>,
}

impl XApiClient {
    pub fn new(bearer_token: String) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {bearer_token}").parse()?,
        );
        headers.insert(header::CONTENT_TYPE, "application/json".parse()?);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            // Keep the client itself timeout-free so streaming connections can stay open.
            // Non-stream requests use per-request timeouts.
            .connect_timeout(STREAM_CONNECT_TIMEOUT)
            .tcp_keepalive(STREAM_TCP_KEEPALIVE)
            .build()
            .context("failed to build reqwest client")?;

        Ok(Self { http })
    }

    pub async fn add_rule(&self, value: String, tag: String) -> Result<String> {
        let body = AddRuleBody {
            add: vec![AddRule { value, tag }],
        };

        let response = self
            .http
            .post("https://api.x.com/2/tweets/search/stream/rules")
            .timeout(API_REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
            .context("failed to call add rule endpoint")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("add rule failed ({status}): {body}");
        }

        let parsed = response
            .json::<AddRuleResponse>()
            .await
            .context("failed to parse add rule response")?;

        if let Some(errors) = parsed.errors {
            let rendered = format_errors(&errors);
            anyhow::bail!("add rule returned errors: {rendered}");
        }

        let rule_id = parsed
            .data
            .and_then(|rules| rules.into_iter().next())
            .map(|rule| rule.id)
            .context("add rule response missing rule id")?;

        Ok(rule_id)
    }

    pub async fn delete_rule(&self, id: String) -> Result<()> {
        self.delete_rule_ids(vec![id]).await.map(|_| ())
    }

    pub async fn list_rules(&self) -> Result<Vec<StreamRule>> {
        let response = self
            .http
            .get("https://api.x.com/2/tweets/search/stream/rules")
            .timeout(API_REQUEST_TIMEOUT)
            .send()
            .await
            .context("failed to call list rules endpoint")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("list rules failed ({status}): {body}");
        }

        let parsed = response
            .json::<GetRulesResponse>()
            .await
            .context("failed to parse list rules response")?;

        if let Some(errors) = parsed.errors {
            let rendered = format_errors(&errors);
            anyhow::bail!("list rules returned errors: {rendered}");
        }

        Ok(parsed.data.unwrap_or_default())
    }

    pub async fn delete_rules_by_tag(&self, tag: &str) -> Result<usize> {
        let ids = self
            .list_rules()
            .await?
            .into_iter()
            .filter(|rule| rule.tag.as_deref() == Some(tag))
            .map(|rule| rule.id)
            .collect::<Vec<_>>();
        self.delete_rule_ids(ids).await
    }

    pub async fn delete_rules_by_tag_prefix(&self, prefix: &str) -> Result<usize> {
        let ids = self
            .list_rules()
            .await?
            .into_iter()
            .filter_map(|rule| {
                let tag = rule.tag?;
                if tag.starts_with(prefix) {
                    Some(rule.id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        self.delete_rule_ids(ids).await
    }

    async fn delete_rule_ids(&self, ids: Vec<String>) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }

        let body = DeleteRuleBody {
            delete: DeleteRule { ids },
        };

        let response = self
            .http
            .post("https://api.x.com/2/tweets/search/stream/rules")
            .timeout(API_REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
            .context("failed to call delete rule endpoint")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("delete rule failed ({status}): {body}");
        }

        Ok(body.delete.ids.len())
    }

    pub async fn terminate_all_connections(&self) -> Result<String> {
        let response = self
            .http
            .delete("https://api.x.com/2/connections/all")
            .timeout(API_REQUEST_TIMEOUT)
            .send()
            .await
            .context("failed to call terminate connections endpoint")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("terminate connections failed ({status}): {body}");
        }

        let body = response.text().await.unwrap_or_default();
        if body.trim().is_empty() {
            return Ok("terminated all active stream connections".to_string());
        }

        let parsed = serde_json::from_str::<TerminateConnectionsResponse>(&body)
            .context("failed to parse terminate connections response")?;

        let warnings = parsed
            .errors
            .as_ref()
            .filter(|errors| !errors.is_empty())
            .map(|errors| format_errors(errors));

        let summary = if let Some(data) = parsed.data {
            if data.successful_kills.is_some() || data.failed_kills.is_some() {
                let successful = data.successful_kills.unwrap_or(0);
                let failed = data.failed_kills.unwrap_or(0);
                format!("terminate-all complete (successful: {successful}, failed: {failed})")
            } else if data.killed_connections == Some(false) {
                "terminate-all complete (no active stream connections)".to_string()
            } else {
                "terminated all active stream connections".to_string()
            }
        } else if let Some(rendered) = &warnings {
            anyhow::bail!("terminate-all returned errors: {rendered}");
        } else {
            "terminated all active stream connections".to_string()
        };

        if let Some(warning_text) = warnings {
            Ok(format!("{summary}; warnings: {warning_text}"))
        } else {
            Ok(summary)
        }
    }

    pub async fn stream_loop(
        self,
        tx: UnboundedSender<AppMsg>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        let mut retry_seconds = 2u64;
        let mut missing_rules_message_sent = false;
        let mut provisioning_message_sent = false;
        let mut too_many_connections_message_sent = false;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let _ = tx.send(AppMsg::Info("connecting to filtered stream".to_string()));

            match self.stream_once(&tx, &mut shutdown_rx).await {
                Ok(()) => {
                    let _ = tx.send(AppMsg::StreamConnectionState(false));
                    let _ = tx.send(AppMsg::Info("stream stopped".to_string()));
                    break;
                }
                Err(error) => {
                    let _ = tx.send(AppMsg::StreamConnectionState(false));
                    let err_text = error.to_string();
                    if err_text == "no stream rules configured" {
                        if !missing_rules_message_sent {
                            let _ = tx.send(AppMsg::Info(
                                "No stream rules are configured yet. Press 'a' to add a monitor."
                                    .to_string(),
                            ));
                            missing_rules_message_sent = true;
                        }
                        provisioning_message_sent = false;
                        tokio::select! {
                            _ = shutdown_rx.changed() => {
                                break;
                            }
                            _ = sleep(Duration::from_secs(5)) => {}
                        }
                        continue;
                    }

                    if err_text == "subscription provisioning in progress" {
                        if !provisioning_message_sent {
                            let _ = tx.send(AppMsg::Info(
                                "𝕏 is provisioning your subscription. Retrying stream in 60s."
                                    .to_string(),
                            ));
                            provisioning_message_sent = true;
                        }
                        missing_rules_message_sent = false;
                        too_many_connections_message_sent = false;
                        tokio::select! {
                            _ = shutdown_rx.changed() => {
                                break;
                            }
                            _ = sleep(Duration::from_secs(60)) => {}
                        }
                        continue;
                    }

                    if err_text == "too many stream connections" {
                        if !too_many_connections_message_sent {
                            let _ = tx.send(AppMsg::Info(
                                "𝕏 reports max active stream connections. Close other clients or press 'x' to terminate all, then wait for reconnect."
                                    .to_string(),
                            ));
                            too_many_connections_message_sent = true;
                        }
                        missing_rules_message_sent = false;
                        provisioning_message_sent = false;
                        retry_seconds = 60;
                        tokio::select! {
                            _ = shutdown_rx.changed() => {
                                break;
                            }
                            _ = sleep(Duration::from_secs(60)) => {}
                        }
                        continue;
                    }

                    missing_rules_message_sent = false;
                    provisioning_message_sent = false;
                    too_many_connections_message_sent = false;
                    let _ = tx.send(AppMsg::Error(format!("stream disconnected: {error}")));
                    let _ = tx.send(AppMsg::Info(format!(
                        "retrying stream connection in {retry_seconds}s"
                    )));
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            break;
                        }
                        _ = sleep(Duration::from_secs(retry_seconds)) => {}
                    }
                    retry_seconds = (retry_seconds * 2).min(60);
                }
            }
        }
    }

    async fn stream_once(
        &self,
        tx: &UnboundedSender<AppMsg>,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) -> Result<()> {
        let response = self
            .http
            .get("https://api.x.com/2/tweets/search/stream")
            .query(&[
                ("expansions", "author_id"),
                ("tweet.fields", "author_id,created_at"),
                ("user.fields", "username"),
            ])
            .send()
            .await
            .context("failed to connect to stream endpoint")?;

        if response.status() == StatusCode::UNAUTHORIZED {
            anyhow::bail!("stream unauthorized (check 𝕏 bearer token permissions)");
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status == StatusCode::CONFLICT
                && (body.contains("RuleConfigurationIssue")
                    || body
                        .to_ascii_lowercase()
                        .contains("must define rules using the post /2/tweets/search/stream/rules"))
            {
                anyhow::bail!("no stream rules configured");
            }
            if status == StatusCode::SERVICE_UNAVAILABLE
                && (body.contains("ProvisioningSubscription")
                    || body
                        .to_ascii_lowercase()
                        .contains("subscription change is currently being provisioned"))
            {
                anyhow::bail!("subscription provisioning in progress");
            }
            if status == StatusCode::TOO_MANY_REQUESTS
                && (body.contains("TooManyConnections")
                    || body
                        .to_ascii_lowercase()
                        .contains("maximum allowed connection"))
            {
                anyhow::bail!("too many stream connections");
            }
            anyhow::bail!("stream request failed ({status}): {body}");
        }

        let stream = response.bytes_stream().map_err(std::io::Error::other);

        let reader = StreamReader::new(stream);
        let mut lines = BufReader::new(reader).lines();

        let _ = tx.send(AppMsg::StreamConnectionState(true));
        let _ = tx.send(AppMsg::Info("stream connected".to_string()));

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    return Ok(());
                }
                line = lines.next_line() => {
                    let Some(line) = line.context("stream read failure")? else {
                        anyhow::bail!("stream ended by remote host");
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    handle_stream_line(tx, &line)?;
                }
            }
        }
    }
}

fn handle_stream_line(tx: &UnboundedSender<AppMsg>, line: &str) -> Result<()> {
    let parsed: StreamEnvelope = serde_json::from_str(line)
        .with_context(|| format!("failed to parse stream message: {line}"))?;

    if let Some(errors) = parsed.errors {
        let rendered = format_errors(&errors);
        let _ = tx.send(AppMsg::Error(format!("stream response errors: {rendered}")));
        return Ok(());
    }

    if let Some(data) = parsed.data {
        let usernames = parsed
            .includes
            .and_then(|includes| includes.users)
            .unwrap_or_default()
            .into_iter()
            .map(|user| (user.id, user.username))
            .collect::<HashMap<_, _>>();

        let author_username = data
            .author_id
            .as_ref()
            .and_then(|author_id| usernames.get(author_id).cloned());

        let matching_tags = parsed
            .matching_rules
            .unwrap_or_default()
            .into_iter()
            .filter_map(|rule| rule.tag)
            .collect::<Vec<_>>();

        let post = StreamPost {
            id: data.id,
            author_id: data.author_id,
            author_username,
            text: data.text,
            matching_tags,
        };
        let _ = tx.send(AppMsg::StreamPost(post));
    }

    Ok(())
}

fn format_errors(errors: &[ApiError]) -> String {
    errors
        .iter()
        .map(|error| {
            let mut parts = Vec::new();
            if let Some(title) = &error.title {
                parts.push(title.clone());
            }
            if let Some(detail) = &error.detail {
                parts.push(detail.clone());
            }
            if let Some(value) = &error.value {
                parts.push(format!("value={value}"));
            }
            if parts.is_empty() {
                "unknown error".to_string()
            } else {
                parts.join(" | ")
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}
