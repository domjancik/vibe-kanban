use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent};
use futures_util::StreamExt;
use ratatui::{DefaultTerminal, layout::Rect};
use tokio::{select, time::interval};

use crate::{
    app_state::App,
    input::{TerminalInput, map_app_key, map_terminal_key},
    model::{Focus, Pane},
    ui::rect_from_size,
};

impl App {
    pub async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut events = EventStream::new();
        let mut ticker = interval(Duration::from_millis(150));
        self.status = format!("Connected to {}", self.api.base_url);

        self.ensure_workspace_selected(rect_from_size(terminal.size()?));

        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            select! {
                _ = ticker.tick() => {
                    self.flush_notes_if_needed().await;
                    self.flush_draft_if_needed().await;
                }
                Some(event) = self.rx.recv() => {
                    self.handle_net_event(event, rect_from_size(terminal.size()?)).await;
                }
                Some(Ok(event)) = events.next() => {
                    if let CrosstermEvent::Key(key) = event {
                        self.handle_key(key, rect_from_size(terminal.size()?)).await;
                    }
                }
            }
        }

        self.subscriptions.abort();
        if let Some(handle) = self.conversation_loader.take() {
            handle.abort();
        }
        for handle in self.workspace_streams.drain(..) {
            handle.abort();
        }
        for handle in self.summary_streams.drain(..) {
            handle.abort();
        }
        Ok(())
    }

    async fn handle_key(&mut self, key: KeyEvent, size: Rect) {
        if self.agent_picker.is_some() {
            self.handle_agent_picker_key(key);
            return;
        }
        if self.session_rename.is_some() {
            self.handle_session_rename_key(key).await;
            return;
        }

        if self.bundle.terminal.input_mode && self.selected_pane == Pane::Terminal {
            match map_terminal_key(key) {
                Some(TerminalInput::ExitInputMode) => {
                    self.bundle.terminal.input_mode = false;
                    self.focus = Focus::Main;
                    self.status = "Left terminal input mode".to_string();
                }
                Some(TerminalInput::SendBytes(bytes)) => self.send_terminal_input(bytes),
                None => {}
            }
            return;
        }

        if self.focus == Focus::Composer {
            match self.selected_pane {
                Pane::Chat => {
                    self.handle_editor_key(key, false).await;
                    return;
                }
                Pane::Notes => {
                    self.handle_editor_key(key, true).await;
                    return;
                }
                _ => {}
            }
        }

        if let Some(intent) = map_app_key(key, self.creating_new_session, &self.selected_pane) {
            self.handle_app_intent(intent, size).await;
        }
    }
}
