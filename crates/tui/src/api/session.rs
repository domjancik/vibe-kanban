use std::time::Duration;

use anyhow::Result;
use db::models::{scratch::DraftFollowUpData, session::Session};
use futures_util::StreamExt;
use json_patch::Patch;
use serde_json::{Value, json};
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

use super::transport::{run_patch_stream, ws_base};
use crate::{
    api::{Api, SCRATCH_TYPE_DRAFT_FOLLOW_UP, WorkspaceSubscriptions, net_error},
    model::{
        CreateSessionRequest, FollowUpRequest, LogEntriesState, NetEvent, PatchType, QueueStatus,
        ScratchPayload, ScratchRecord, ScratchStreamState, UpdateScratchPayload,
        UpdateScratchRequest, UpdateSessionRequest,
    },
};

impl Api {
    pub fn replace_draft_stream(
        &self,
        scratch_id: Option<Uuid>,
        tx: UnboundedSender<NetEvent>,
        subscriptions: &mut WorkspaceSubscriptions,
    ) {
        if let Some(handle) = subscriptions.draft.take() {
            handle.abort();
        }
        if let Some(scratch_id) = scratch_id {
            subscriptions.draft = Some(spawn_draft_stream(self.clone(), scratch_id, tx));
        }
    }

    pub async fn rename_session(&self, session_id: Uuid, name: String) -> Result<Session> {
        self.put(
            &format!("/api/sessions/{session_id}"),
            &UpdateSessionRequest { name: Some(name) },
        )
        .await
    }

    pub fn load_queue_status(&self, session_id: Uuid, tx: UnboundedSender<NetEvent>) {
        let api = self.clone();
        tokio::spawn(async move {
            match api
                .get::<QueueStatus>(&format!("/api/sessions/{session_id}/queue"))
                .await
            {
                Ok(status) => {
                    let _ = tx.send(NetEvent::QueueLoaded { session_id, status });
                }
                Err(error) => {
                    let _ = tx.send(net_error(
                        format!("load queue status for session {session_id}"),
                        error,
                    ));
                }
            }
        });
    }

    pub async fn save_follow_up_draft(
        &self,
        scratch_id: Uuid,
        draft: DraftFollowUpData,
    ) -> Result<()> {
        let request = UpdateScratchRequest {
            payload: UpdateScratchPayload::DraftFollowUp(draft),
        };
        let _: ScratchRecord = self
            .put(
                &format!("/api/scratch/{SCRATCH_TYPE_DRAFT_FOLLOW_UP}/{scratch_id}"),
                &request,
            )
            .await?;
        Ok(())
    }

    pub async fn delete_follow_up_draft(&self, scratch_id: Uuid) -> Result<()> {
        self.delete_empty(&format!(
            "/api/scratch/{SCRATCH_TYPE_DRAFT_FOLLOW_UP}/{scratch_id}"
        ))
        .await
    }

    pub async fn queue_follow_up(
        &self,
        session_id: Uuid,
        draft: DraftFollowUpData,
    ) -> Result<QueueStatus> {
        self.post(
            &format!("/api/sessions/{session_id}/queue"),
            &serde_json::json!({
                "message": draft.message,
                "executor_config": draft.executor_config,
            }),
        )
        .await
    }

    pub async fn cancel_queued_follow_up(&self, session_id: Uuid) -> Result<QueueStatus> {
        self.delete(&format!("/api/sessions/{session_id}/queue"))
            .await
    }

    pub async fn fetch_process_log_snapshot(&self, process_id: Uuid) -> Result<Vec<PatchType>> {
        let endpoint = format!(
            "{}/api/execution-processes/{process_id}/normalized-logs/ws",
            ws_base(&self.base_url)
        );
        let (stream, _) = connect_async(endpoint.as_str()).await?;
        let (_, mut read) = stream.split();
        let mut state = json!({ "entries": [] });
        let mut received_patch = false;

        while let Some(message) = tokio::time::timeout(Duration::from_millis(1500), read.next())
            .await
            .unwrap_or(None)
        {
            let message = message?;
            let Message::Text(text) = message else {
                continue;
            };
            if text.contains(r#""Ready":true"#) || text.contains(r#""finished":true"#) {
                if received_patch {
                    break;
                }
                continue;
            }
            let payload: Value = serde_json::from_str(&text)?;
            if let Some(patch_value) = payload.get("JsonPatch") {
                let patch: Patch = serde_json::from_value(patch_value.clone())?;
                json_patch::patch(&mut state, &patch)?;
                received_patch = true;
            }
        }

        let typed: LogEntriesState = serde_json::from_value(state)?;
        Ok(typed.entries)
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

fn spawn_draft_stream(api: Api, scratch_id: Uuid, tx: UnboundedSender<NetEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}/api/scratch/{SCRATCH_TYPE_DRAFT_FOLLOW_UP}/{scratch_id}/stream/ws",
            ws_base(&api.base_url)
        );
        let result = run_patch_stream::<ScratchStreamState, _>(
            endpoint,
            json!({ "scratch": null }),
            move |state| {
                let draft = state.scratch.and_then(|scratch| match scratch.payload {
                    ScratchPayload::DraftFollowUp(data) => Some(data),
                    _ => None,
                });
                NetEvent::DraftLoaded { scratch_id, draft }
            },
            tx.clone(),
        )
        .await;
        if let Err(error) = result {
            let _ = tx.send(net_error(
                format!("draft stream for scratch {scratch_id}"),
                error,
            ));
        }
    })
}
