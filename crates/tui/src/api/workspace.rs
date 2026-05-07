use std::collections::HashMap;

use anyhow::Result;
use db::models::{session::Session, workspace::Workspace};
use serde_json::json;
use tokio::{
    sync::mpsc::{UnboundedSender, unbounded_channel},
    task::JoinHandle,
    time::sleep,
};
use uuid::Uuid;

use super::{
    terminal::spawn_terminal_stream,
    transport::{run_patch_stream, ws_base},
};
use crate::{
    api::{Api, SCRATCH_TYPE_WORKSPACE_NOTES, WorkspaceSubscriptions},
    model::{
        DiffStreamState, ExecutionProcessesState, ExecutorDiscoveryStreamState, LogEntriesState,
        NetEvent, PatchType, RepoBranchStatus, ScratchPayload, ScratchRecord, ScratchStreamState,
        UpdateScratchPayload, UpdateScratchRequest, UpdateWorkspaceRequest, WorkspaceStreamState,
        WorkspaceSummaryRequest, WorkspaceSummaryResponse,
    },
};

impl Api {
    pub fn spawn_workspace_streams(&self, tx: UnboundedSender<NetEvent>) -> Vec<JoinHandle<()>> {
        vec![
            spawn_workspace_stream(self.clone(), false, tx.clone()),
            spawn_workspace_stream(self.clone(), true, tx),
        ]
    }

    pub fn spawn_summary_pollers(&self, tx: UnboundedSender<NetEvent>) -> Vec<JoinHandle<()>> {
        vec![
            spawn_summary_poller(self.clone(), false, tx.clone()),
            spawn_summary_poller(self.clone(), true, tx),
        ]
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
                .get::<Vec<RepoBranchStatus>>(&format!("/api/workspaces/{workspace_id}/git/status"))
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
                .get::<ScratchRecord>(&format!(
                    "/api/scratch/{SCRATCH_TYPE_WORKSPACE_NOTES}/{workspace_id}"
                ))
                .await
            {
                Ok(scratch) => {
                    let notes = match scratch.payload {
                        ScratchPayload::WorkspaceNotes(data) => data.content,
                        ScratchPayload::DraftFollowUp(_) => String::new(),
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
                &format!("/api/scratch/{SCRATCH_TYPE_WORKSPACE_NOTES}/{workspace_id}"),
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
        let request = crate::model::OpenEditorRequest {
            editor_type: None,
            file_path: None,
        };
        let _: serde_json::Value = self
            .post(
                &format!("/api/workspaces/{workspace_id}/integration/editor/open"),
                &request,
            )
            .await?;
        Ok(())
    }
}

fn spawn_workspace_stream(
    api: Api,
    archived: bool,
    tx: UnboundedSender<NetEvent>,
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
                    let _ = tx.send(NetEvent::Summaries(response.summaries));
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::Error(error.to_string()));
                }
            }
            sleep(std::time::Duration::from_secs(15)).await;
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
    })
}

fn spawn_notes_stream(
    api: Api,
    workspace_id: Uuid,
    tx: UnboundedSender<NetEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/scratch/{SCRATCH_TYPE_WORKSPACE_NOTES}/{workspace_id}/stream/ws",
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
                        ScratchPayload::DraftFollowUp(_) => None,
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
