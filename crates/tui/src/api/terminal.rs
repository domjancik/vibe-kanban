use std::sync::Arc;

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    sync::{
        Mutex,
        mpsc::{UnboundedReceiver, UnboundedSender},
    },
    task::JoinHandle,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

use super::transport::ws_base;
use crate::{
    api::{Api, TerminalCommand},
    model::NetEvent,
};

pub fn spawn_terminal_stream(
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
