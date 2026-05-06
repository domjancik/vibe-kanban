use std::{collections::HashMap, fs, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use db::models::{session::Session, workspace::Workspace};
use futures_util::{SinkExt, StreamExt};
use json_patch::Patch;
use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    sync::{
        Mutex,
        mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    },
    task::JoinHandle,
    time::sleep,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

use crate::model::{
    ApiEnvelope, CreateSessionRequest, DiffStreamState, ExecutionProcessesState,
    ExecutorDiscoveryStreamState, FollowUpRequest, LogEntriesState, NetEvent, OpenEditorRequest,
    PatchType, ScratchPayload, ScratchRecord, ScratchStreamState, StreamKind, UpdateScratchPayload,
    UpdateScratchRequest, UpdateWorkspaceRequest, UserSystemInfo, WorkspaceStreamState,
    WorkspaceSummaryRequest, WorkspaceSummaryResponse,
};

#[derive(Clone)]
pub struct Api {
    pub base_url: String,
    client: Client,
}

#[derive(Default)]
pub struct WorkspaceSubscriptions {
    pub diff: Option<JoinHandle<()>>,
    pub notes: Option<JoinHandle<()>>,
    pub processes: Option<JoinHandle<()>>,
    pub logs: Option<JoinHandle<()>>,
    pub discovery: Option<JoinHandle<()>>,
    pub terminal: Option<JoinHandle<()>>,
    pub terminal_tx: Option<UnboundedSender<TerminalCommand>>,
}

pub enum TerminalCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
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
        let response = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .send()
            .await
            .with_context(|| format!("GET {path} failed"))?;
        parse_api_response(response).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {path} failed"))?;
        parse_api_response(response).await
    }

    pub async fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let response = self
            .client
            .put(format!("{}{}", self.base_url, path))
            .json(body)
            .send()
            .await
            .with_context(|| format!("PUT {path} failed"))?;
        parse_api_response(response).await
    }

    pub async fn post_no_content<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let _: Value = self.post(path, body).await?;
        Ok(())
    }

    pub async fn post_empty(&self, path: &str) -> Result<()> {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .send()
            .await
            .with_context(|| format!("POST {path} failed"))?;
        let _: Value = parse_api_response(response).await?;
        Ok(())
    }

    pub fn spawn_workspace_streams(&self, tx: UnboundedSender<NetEvent>) -> Vec<JoinHandle<()>> {
        vec![
            spawn_workspace_stream(
                self.clone(),
                false,
                tx.clone(),
                StreamKind::ActiveWorkspaces,
            ),
            spawn_workspace_stream(self.clone(), true, tx, StreamKind::ArchivedWorkspaces),
        ]
    }

    pub fn spawn_summary_pollers(&self, tx: UnboundedSender<NetEvent>) -> Vec<JoinHandle<()>> {
        vec![
            spawn_summary_poller(self.clone(), false, tx.clone()),
            spawn_summary_poller(self.clone(), true, tx),
        ]
    }

    pub fn load_user_system_info(&self, tx: UnboundedSender<NetEvent>) {
        let api = self.clone();
        tokio::spawn(async move {
            match api.get::<UserSystemInfo>("/api/info").await {
                Ok(info) => {
                    let _ = tx.send(NetEvent::UserSystemLoaded(info));
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::Error(error.to_string()));
                }
            }
        });
    }

    pub fn load_workspace(&self, workspace_id: Uuid, tx: UnboundedSender<NetEvent>) {
        let api = self.clone();
        let tx_workspace = tx.clone();
        tokio::spawn(async move {
            match api
                .get::<Workspace>(&format!("/api/workspaces/{workspace_id}"))
                .await
            {
                Ok(workspace) => {
                    let _ = tx_workspace.send(NetEvent::WorkspaceLoaded(workspace));
                }
                Err(error) => {
                    let _ = tx_workspace.send(NetEvent::Error(error.to_string()));
                }
            }
        });

        let api = self.clone();
        let tx_sessions = tx.clone();
        tokio::spawn(async move {
            match api
                .get::<Vec<Session>>(&format!("/api/sessions?workspace_id={workspace_id}"))
                .await
            {
                Ok(sessions) => {
                    let _ = tx_sessions.send(NetEvent::SessionsLoaded {
                        workspace_id,
                        sessions,
                    });
                }
                Err(error) => {
                    let _ = tx_sessions.send(NetEvent::Error(error.to_string()));
                }
            }
        });

        let api = self.clone();
        let tx_repos = tx.clone();
        tokio::spawn(async move {
            match api
                .get::<Vec<db::models::workspace_repo::RepoWithTargetBranch>>(&format!(
                    "/api/workspaces/{workspace_id}/repos"
                ))
                .await
            {
                Ok(repos) => {
                    let _ = tx_repos.send(NetEvent::ReposLoaded {
                        workspace_id,
                        repos,
                    });
                }
                Err(error) => {
                    let _ = tx_repos.send(NetEvent::Error(error.to_string()));
                }
            }
        });

        let api = self.clone();
        let tx_git = tx.clone();
        tokio::spawn(async move {
            match api
                .get::<Vec<crate::model::RepoBranchStatus>>(&format!(
                    "/api/workspaces/{workspace_id}/git/status"
                ))
                .await
            {
                Ok(statuses) => {
                    let _ = tx_git.send(NetEvent::GitStatusLoaded {
                        workspace_id,
                        statuses,
                    });
                }
                Err(error) => {
                    let _ = tx_git.send(NetEvent::Error(error.to_string()));
                }
            }
        });

        let api = self.clone();
        tokio::spawn(async move {
            match api
                .get::<ScratchRecord>(&format!("/api/scratch/workspace_notes/{workspace_id}"))
                .await
            {
                Ok(scratch) => {
                    let notes = match scratch.payload {
                        ScratchPayload::WorkspaceNotes(data) => data.content,
                        ScratchPayload::Other => String::new(),
                    };
                    let _ = tx.send(NetEvent::NotesLoaded {
                        workspace_id,
                        notes,
                    });
                }
                Err(_) => {
                    let _ = tx.send(NetEvent::NotesLoaded {
                        workspace_id,
                        notes: String::new(),
                    });
                }
            }
        });
    }

    pub fn replace_workspace_subscriptions(
        &self,
        workspace_id: Uuid,
        session_id: Option<Uuid>,
        selected_process_id: Option<Uuid>,
        size: (u16, u16),
        tx: UnboundedSender<NetEvent>,
        subscriptions: &mut WorkspaceSubscriptions,
    ) {
        subscriptions.abort();
        subscriptions.diff = Some(spawn_diff_stream(self.clone(), workspace_id, tx.clone()));
        subscriptions.notes = Some(spawn_notes_stream(self.clone(), workspace_id, tx.clone()));
        if let Some(session_id) = session_id {
            subscriptions.processes =
                Some(spawn_process_stream(self.clone(), session_id, tx.clone()));
        }
        if let Some(process_id) = selected_process_id {
            subscriptions.logs = Some(spawn_logs_stream(self.clone(), process_id, tx.clone()));
        }
        if let Some(handle) = subscriptions.discovery.take() {
            handle.abort();
        }
        let (terminal_tx, terminal_rx) = unbounded_channel();
        subscriptions.terminal_tx = Some(terminal_tx);
        subscriptions.terminal = Some(spawn_terminal_stream(
            self.clone(),
            workspace_id,
            size,
            tx,
            terminal_rx,
        ));
    }

    pub fn replace_process_stream(
        &self,
        session_id: Option<Uuid>,
        tx: UnboundedSender<NetEvent>,
        subscriptions: &mut WorkspaceSubscriptions,
    ) {
        if let Some(handle) = subscriptions.processes.take() {
            handle.abort();
        }

        if let Some(session_id) = session_id {
            subscriptions.processes =
                Some(spawn_process_stream(self.clone(), session_id, tx.clone()));
        }
    }

    pub fn replace_logs_stream(
        &self,
        selected_process_id: Option<Uuid>,
        tx: UnboundedSender<NetEvent>,
        subscriptions: &mut WorkspaceSubscriptions,
    ) {
        if let Some(handle) = subscriptions.logs.take() {
            handle.abort();
        }
        if let Some(process_id) = selected_process_id {
            subscriptions.logs = Some(spawn_logs_stream(self.clone(), process_id, tx.clone()));
        }
    }

    pub fn replace_discovery_stream(
        &self,
        executor: executors::executors::BaseCodingAgent,
        workspace_id: Option<Uuid>,
        session_id: Option<Uuid>,
        tx: UnboundedSender<NetEvent>,
        subscriptions: &mut WorkspaceSubscriptions,
    ) {
        if let Some(handle) = subscriptions.discovery.take() {
            handle.abort();
        }
        subscriptions.discovery = Some(spawn_discovery_stream(
            self.clone(),
            executor,
            workspace_id,
            session_id,
            tx,
        ));
    }

    pub async fn save_notes(&self, workspace_id: Uuid, notes: String) -> Result<()> {
        let request = UpdateScratchRequest {
            payload: UpdateScratchPayload::WorkspaceNotes(
                db::models::scratch::WorkspaceNotesData { content: notes },
            ),
        };
        let _: ScratchRecord = self
            .put(
                &format!("/api/scratch/workspace_notes/{workspace_id}"),
                &request,
            )
            .await?;
        Ok(())
    }

    pub async fn toggle_pinned(&self, workspace_id: Uuid, pinned: bool) -> Result<()> {
        let request = UpdateWorkspaceRequest {
            archived: None,
            pinned: Some(pinned),
            name: None,
        };
        let _: Workspace = self
            .put(&format!("/api/workspaces/{workspace_id}"), &request)
            .await?;
        Ok(())
    }

    pub async fn toggle_archived(&self, workspace_id: Uuid, archived: bool) -> Result<()> {
        let request = UpdateWorkspaceRequest {
            archived: Some(archived),
            pinned: None,
            name: None,
        };
        let _: Workspace = self
            .put(&format!("/api/workspaces/{workspace_id}"), &request)
            .await?;
        Ok(())
    }

    pub async fn stop_workspace(&self, workspace_id: Uuid) -> Result<()> {
        self.post_empty(&format!("/api/workspaces/{workspace_id}/execution/stop"))
            .await
    }

    pub async fn start_dev_server(&self, workspace_id: Uuid) -> Result<()> {
        self.post_empty(&format!(
            "/api/workspaces/{workspace_id}/execution/dev-server/start"
        ))
        .await
    }

    pub async fn run_cleanup(&self, workspace_id: Uuid) -> Result<()> {
        self.post_empty(&format!("/api/workspaces/{workspace_id}/execution/cleanup"))
            .await
    }

    pub async fn open_editor(&self, workspace_id: Uuid) -> Result<()> {
        let request = OpenEditorRequest {
            editor_type: None,
            file_path: None,
        };
        let _: Value = self
            .post(
                &format!("/api/workspaces/{workspace_id}/integration/editor/open"),
                &request,
            )
            .await?;
        Ok(())
    }

    pub async fn send_prompt(
        &self,
        workspace_id: Uuid,
        session: Option<Session>,
        prompt: String,
        executor_config: executors::profile::ExecutorConfig,
    ) -> Result<Uuid> {
        let session = if let Some(session) = session {
            session
        } else {
            let executor = Some(executor_config.executor.to_string());
            self.post::<_, Session>(
                "/api/sessions",
                &CreateSessionRequest {
                    workspace_id,
                    executor,
                    name: None,
                },
            )
            .await?
        };

        let _: db::models::execution_process::ExecutionProcess = self
            .post(
                &format!("/api/sessions/{}/follow-up", session.id),
                &FollowUpRequest {
                    prompt,
                    executor_config,
                    retry_process_id: None,
                    force_when_dirty: None,
                    perform_git_reset: None,
                },
            )
            .await?;
        Ok(session.id)
    }
}

impl WorkspaceSubscriptions {
    pub fn abort(&mut self) {
        for handle in [
            self.diff.take(),
            self.notes.take(),
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
    }
}

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

async fn parse_api_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let body = response.text().await?;
    let envelope: ApiEnvelope<T> =
        serde_json::from_str(&body).with_context(|| format!("invalid API response: {body}"))?;
    if !status.is_success() || !envelope.success {
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

fn spawn_workspace_stream(
    api: Api,
    archived: bool,
    tx: UnboundedSender<NetEvent>,
    kind: StreamKind,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/workspaces/streams/ws?archived={archived}",
            ws_base(&api.base_url)
        );
        let result = run_patch_stream::<WorkspaceStreamState, _>(
            endpoint,
            json!({ "workspaces": {} }),
            |state| {
                if archived {
                    NetEvent::ArchivedWorkspaces(state)
                } else {
                    NetEvent::ActiveWorkspaces(state)
                }
            },
            tx.clone(),
        )
        .await;
        if let Err(error) = result {
            let _ = tx.send(NetEvent::Error(error.to_string()));
        }
        let _ = tx.send(NetEvent::StreamClosed(kind));
    })
}

fn spawn_summary_poller(api: Api, archived: bool, tx: UnboundedSender<NetEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match api
                .post::<_, WorkspaceSummaryResponse>(
                    "/api/workspaces/summaries",
                    &WorkspaceSummaryRequest { archived },
                )
                .await
            {
                Ok(response) => {
                    let _ = tx.send(NetEvent::Summaries {
                        archived,
                        data: response.summaries,
                    });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::Error(error.to_string()));
                }
            }
            sleep(Duration::from_secs(15)).await;
        }
    })
}

fn spawn_diff_stream(
    api: Api,
    workspace_id: Uuid,
    tx: UnboundedSender<NetEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/workspaces/{workspace_id}/git/diff/ws",
            ws_base(&api.base_url)
        );
        let result = run_patch_stream::<DiffStreamState, _>(
            endpoint,
            json!({ "entries": {} }),
            move |state| {
                let diffs = state
                    .entries
                    .into_values()
                    .flat_map(HashMap::into_values)
                    .filter_map(|patch| match patch {
                        PatchType::Diff(diff) => Some(diff),
                        _ => None,
                    })
                    .collect();
                NetEvent::DiffsUpdated {
                    workspace_id,
                    diffs,
                }
            },
            tx.clone(),
        )
        .await;
        if let Err(error) = result {
            let _ = tx.send(NetEvent::Error(error.to_string()));
        }
        let _ = tx.send(NetEvent::StreamClosed(StreamKind::Diffs(workspace_id)));
    })
}

fn spawn_notes_stream(
    api: Api,
    workspace_id: Uuid,
    tx: UnboundedSender<NetEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/scratch/workspace_notes/{workspace_id}/stream/ws",
            ws_base(&api.base_url)
        );
        let result = run_patch_stream::<ScratchStreamState, _>(
            endpoint,
            json!({ "scratch": null }),
            move |state| {
                let notes = state
                    .scratch
                    .and_then(|scratch| match scratch.payload {
                        ScratchPayload::WorkspaceNotes(data) => Some(data.content),
                        ScratchPayload::Other => None,
                    })
                    .unwrap_or_default();
                NetEvent::NotesLoaded {
                    workspace_id,
                    notes,
                }
            },
            tx.clone(),
        )
        .await;
        if let Err(error) = result {
            let _ = tx.send(NetEvent::Error(error.to_string()));
        }
        let _ = tx.send(NetEvent::StreamClosed(StreamKind::Notes(workspace_id)));
    })
}

fn spawn_process_stream(
    api: Api,
    session_id: Uuid,
    tx: UnboundedSender<NetEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/execution-processes/stream/session/ws?session_id={session_id}",
            ws_base(&api.base_url)
        );
        let result = run_patch_stream::<ExecutionProcessesState, _>(
            endpoint,
            json!({ "execution_processes": {} }),
            move |state| {
                let processes = state
                    .execution_processes
                    .into_values()
                    .map(|process| (process.id, process))
                    .collect();
                NetEvent::ProcessesUpdated {
                    session_id,
                    processes,
                }
            },
            tx.clone(),
        )
        .await;
        if let Err(error) = result {
            let _ = tx.send(NetEvent::Error(error.to_string()));
        }
        let _ = tx.send(NetEvent::StreamClosed(StreamKind::Processes(session_id)));
    })
}

fn spawn_logs_stream(api: Api, process_id: Uuid, tx: UnboundedSender<NetEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/execution-processes/{process_id}/normalized-logs/ws",
            ws_base(&api.base_url)
        );
        let result = run_patch_stream::<LogEntriesState, _>(
            endpoint,
            json!({ "entries": [] }),
            move |state| NetEvent::LogsUpdated {
                process_id,
                entries: state.entries,
            },
            tx.clone(),
        )
        .await;
        if let Err(error) = result {
            let _ = tx.send(NetEvent::Error(error.to_string()));
        }
        let _ = tx.send(NetEvent::StreamClosed(StreamKind::Logs(process_id)));
    })
}

fn spawn_discovery_stream(
    api: Api,
    executor: executors::executors::BaseCodingAgent,
    workspace_id: Option<Uuid>,
    session_id: Option<Uuid>,
    tx: UnboundedSender<NetEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut endpoint = format!(
            "{}/api/agents/discovered-options/ws?executor={executor}",
            ws_base(&api.base_url)
        );
        if let Some(workspace_id) = workspace_id {
            endpoint.push_str(&format!("&workspace_id={workspace_id}"));
        }
        if let Some(session_id) = session_id {
            endpoint.push_str(&format!("&session_id={session_id}"));
        }

        let result = run_patch_stream::<ExecutorDiscoveryStreamState, _>(
            endpoint,
            json!({
                "options": {
                    "model_selector": {
                        "providers": [],
                        "models": [],
                        "default_model": null,
                        "agents": [],
                        "permissions": []
                    },
                    "slash_commands": [],
                    "loading_models": true,
                    "loading_agents": true,
                    "loading_slash_commands": true,
                    "error": null
                }
            }),
            move |state| NetEvent::ExecutorOptionsUpdated {
                executor,
                options: state.options,
            },
            tx.clone(),
        )
        .await;
        if let Err(error) = result {
            let _ = tx.send(NetEvent::Error(error.to_string()));
        }
    })
}

fn spawn_terminal_stream(
    api: Api,
    workspace_id: Uuid,
    size: (u16, u16),
    tx: UnboundedSender<NetEvent>,
    mut input_rx: UnboundedReceiver<TerminalCommand>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/terminal/ws?workspace_id={workspace_id}&cols={}&rows={}",
            ws_base(&api.base_url),
            size.0,
            size.1
        );

        let result = async {
            let (stream, _) = connect_async(endpoint.as_str()).await?;
            let (write, mut read) = stream.split();
            let write = Arc::new(Mutex::new(write));

            let _ = tx.send(NetEvent::TerminalConnected(workspace_id));

            let writer = {
                let tx = tx.clone();
                let write = Arc::clone(&write);
                tokio::spawn(async move {
                    while let Some(command) = input_rx.recv().await {
                        let json = match command {
                            TerminalCommand::Input(bytes) => json!({
                                "type": "input",
                                "data": STANDARD.encode(bytes),
                            }),
                            TerminalCommand::Resize(cols, rows) => json!({
                                "type": "resize",
                                "cols": cols,
                                "rows": rows,
                            }),
                        };
                        let message = Message::Text(json.to_string());
                        if let Err(error) = write.lock().await.send(message).await {
                            let _ =
                                tx.send(NetEvent::TerminalError(workspace_id, error.to_string()));
                            break;
                        }
                    }
                })
            };

            while let Some(message) = read.next().await {
                let message = message?;
                let Message::Text(text) = message else {
                    continue;
                };
                let payload: Value = serde_json::from_str(&text)?;
                if let Some(data) = payload.get("data").and_then(Value::as_str) {
                    let output = STANDARD.decode(data)?;
                    let _ = tx.send(NetEvent::TerminalOutput(workspace_id, output));
                } else if let Some(error) = payload.get("message").and_then(Value::as_str) {
                    let _ = tx.send(NetEvent::TerminalError(workspace_id, error.to_string()));
                }
            }

            writer.abort();
            Result::<()>::Ok(())
        }
        .await;

        if let Err(error) = result {
            let _ = tx.send(NetEvent::TerminalError(workspace_id, error.to_string()));
        }
    })
}

async fn run_patch_stream<T, F>(
    endpoint: String,
    initial: Value,
    make_event: F,
    tx: UnboundedSender<NetEvent>,
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

fn ws_base(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}
