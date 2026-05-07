use std::time::Duration;

use anyhow::{Context, Error, Result};
use executors::executors::BaseCodingAgent;
use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};
use uuid::Uuid;

pub mod session;
pub mod terminal;
pub mod transport;
pub mod workspace;

pub use self::transport::detect_base_url;
use self::transport::{log_api, parse_api_response};
use crate::model::{NetEvent, UserSystemInfo};

pub(crate) const SCRATCH_TYPE_DRAFT_FOLLOW_UP: &str = "DRAFT_FOLLOW_UP";
pub(crate) const SCRATCH_TYPE_WORKSPACE_NOTES: &str = "WORKSPACE_NOTES";

#[derive(Clone)]
pub struct Api {
    pub base_url: String,
    client: Client,
}

#[derive(Default)]
pub struct WorkspaceSubscriptions {
    pub diff: Option<JoinHandle<()>>,
    pub notes: Option<JoinHandle<()>>,
    pub draft: Option<JoinHandle<()>>,
    pub processes: Option<JoinHandle<()>>,
    pub logs: Option<JoinHandle<()>>,
    pub discovery: Option<JoinHandle<()>>,
    pub discovery_key: Option<DiscoverySubscriptionKey>,
    pub terminal: Option<JoinHandle<()>>,
    pub terminal_tx: Option<UnboundedSender<TerminalCommand>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscoverySubscriptionKey {
    pub executor: BaseCodingAgent,
    pub workspace_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
}

pub enum TerminalCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
}

pub(crate) fn net_error(source: impl AsRef<str>, error: Error) -> NetEvent {
    NetEvent::Error(format!("{}: {error:#}", source.as_ref()))
}

impl Api {
    pub fn new(base_url: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self { base_url, client })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        log_api(format!("HTTP GET {path}"));
        let response = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .send()
            .await
            .with_context(|| format!("GET {path} failed"))?;
        parse_api_response("GET", path, response).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        log_api(format!("HTTP POST {path}"));
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {path} failed"))?;
        parse_api_response("POST", path, response).await
    }

    pub async fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        log_api(format!("HTTP PUT {path}"));
        let response = self
            .client
            .put(format!("{}{}", self.base_url, path))
            .json(body)
            .send()
            .await
            .with_context(|| format!("PUT {path} failed"))?;
        parse_api_response("PUT", path, response).await
    }

    pub async fn post_empty(&self, path: &str) -> Result<()> {
        log_api(format!("HTTP POST {path}"));
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .send()
            .await
            .with_context(|| format!("POST {path} failed"))?;
        let _: Value = parse_api_response("POST", path, response).await?;
        Ok(())
    }

    pub async fn delete_empty(&self, path: &str) -> Result<()> {
        let _: () = self.delete(path).await?;
        Ok(())
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        log_api(format!("HTTP DELETE {path}"));
        let response = self
            .client
            .delete(format!("{}{}", self.base_url, path))
            .send()
            .await
            .with_context(|| format!("DELETE {path} failed"))?;
        parse_api_response("DELETE", path, response).await
    }

    pub fn load_user_system_info(&self, tx: UnboundedSender<NetEvent>) {
        let api = self.clone();
        tokio::spawn(async move {
            match api.get::<UserSystemInfo>("/api/info").await {
                Ok(info) => {
                    let _ = tx.send(NetEvent::UserSystemLoaded(info));
                }
                Err(error) => {
                    let _ = tx.send(net_error("load user system info", error));
                }
            }
        });
    }
}

impl WorkspaceSubscriptions {
    pub fn abort(&mut self) {
        for handle in [
            self.diff.take(),
            self.notes.take(),
            self.draft.take(),
            self.processes.take(),
            self.logs.take(),
            self.discovery.take(),
            self.terminal.take(),
        ]
        .into_iter()
        .flatten()
        {
            handle.abort();
        }
        self.terminal_tx = None;
        self.discovery_key = None;
    }
}
