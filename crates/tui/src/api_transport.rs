use std::{fs, io::Write, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use json_patch::Patch;
use reqwest::Response;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::model::{ApiEnvelope, NetEvent};

pub fn detect_base_url() -> String {
    if let Ok(base) = std::env::var("VK_TUI_BASE_URL") {
        return base;
    }
    if let Ok(port) = std::env::var("BACKEND_PORT").or_else(|_| std::env::var("PORT")) {
        return format!("http://127.0.0.1:{}", port.trim());
    }
    let mut cursor = std::env::current_dir().ok();
    while let Some(dir) = cursor {
        let path = dir.join(".dev-ports.json");
        if path.exists()
            && let Ok(content) = fs::read_to_string(&path)
            && let Ok(json) = serde_json::from_str::<Value>(&content)
            && let Some(port) = json.get("backend").and_then(Value::as_u64)
        {
            return format!("http://127.0.0.1:{port}");
        }
        cursor = dir.parent().map(PathBuf::from);
    }
    "http://127.0.0.1:3001".to_string()
}

pub async fn parse_api_response<T: DeserializeOwned>(
    method: &str,
    path: &str,
    response: Response,
) -> Result<T> {
    let status = response.status();
    let body = response.text().await?;
    log_api(format!("HTTP {method} {path} -> {status}"));
    let envelope: ApiEnvelope<T> = serde_json::from_str(&body).with_context(|| {
        log_api(format!(
            "HTTP {method} {path} parse error body={}",
            truncate_for_log(&body)
        ));
        format!("invalid API response: {body}")
    })?;
    if !status.is_success() || !envelope.success {
        log_api(format!(
            "HTTP {method} {path} api error body={}",
            truncate_for_log(&body)
        ));
        return Err(anyhow!(
            envelope
                .message
                .unwrap_or_else(|| format!("request failed with status {status}"))
        ));
    }
    envelope
        .data
        .ok_or_else(|| anyhow!("API response did not include data"))
}

pub fn log_api(message: String) {
    let path = std::env::var("VK_TUI_API_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("vibe-kanban-tui-api.log"));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[tui-api] {message}");
    }
}

pub async fn run_patch_stream<T, F>(
    endpoint: String,
    initial: Value,
    make_event: F,
    tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
) -> Result<()>
where
    T: DeserializeOwned,
    F: Fn(T) -> NetEvent,
{
    let (stream, _) = connect_async(endpoint.as_str()).await?;
    let (_, mut read) = stream.split();
    let mut state = initial;

    while let Some(message) = read.next().await {
        let message = message?;
        let Message::Text(text) = message else {
            continue;
        };
        if text.contains(r#""Ready":true"#) || text.contains(r#""finished":true"#) {
            continue;
        }
        let payload: Value = serde_json::from_str(&text)?;
        if let Some(patch_value) = payload.get("JsonPatch") {
            let patch: Patch = serde_json::from_value(patch_value.clone())?;
            json_patch::patch(&mut state, &patch)?;
            let typed: T = serde_json::from_value(state.clone())?;
            let _ = tx.send(make_event(typed));
        }
    }
    Ok(())
}

pub fn ws_base(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

fn truncate_for_log(body: &str) -> String {
    const LIMIT: usize = 400;
    if body.len() <= LIMIT {
        body.to_string()
    } else {
        format!("{}...", &body[..LIMIT])
    }
}
