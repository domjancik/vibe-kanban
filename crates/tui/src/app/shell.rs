use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent, KeyEventKind};
use futures_util::StreamExt;
use ratatui::{DefaultTerminal, layout::Rect};
use tokio::{
    select,
    sync::mpsc::error::TryRecvError,
    time::{MissedTickBehavior, interval},
};

use crate::{
    app::App,
    input::{TerminalInput, map_app_key, map_terminal_key},
    model::{Focus, NetEvent, Pane},
    ui::rect_from_size,
};

impl App {
    pub async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        const MAINTENANCE_INTERVAL: Duration = Duration::from_millis(150);
        const BACKGROUND_REDRAW_INTERVAL: Duration = Duration::from_millis(50);
        const INPUT_QUIET_WINDOW: Duration = Duration::from_millis(45);

        let mut events = EventStream::new();
        let mut maintenance = interval(MAINTENANCE_INTERVAL);
        maintenance.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut background_flush = interval(BACKGROUND_REDRAW_INTERVAL);
        background_flush.set_missed_tick_behavior(MissedTickBehavior::Skip);
        self.status = format!("Connected to {}", self.api.base_url);
        let mut draw_requested = true;
        let mut pending_background_redraw = false;
        let mut last_key_event = Instant::now() - INPUT_QUIET_WINDOW;

        self.ensure_workspace_selected(rect_from_size(terminal.size()?));

        while !self.should_quit {
            if draw_requested {
                terminal.draw(|frame| self.render(frame))?;
                draw_requested = false;
                pending_background_redraw = false;
            }
            select! {
                biased;
                Some(Ok(event)) = events.next() => {
                    if let CrosstermEvent::Key(key) = event {
                        if !should_process_key_event(key) {
                            continue;
                        }
                        self.handle_key(key, rect_from_size(terminal.size()?)).await;
                        draw_requested = true;
                        pending_background_redraw = false;
                        last_key_event = Instant::now();
                    }
                }
                Some(event) = self.rx.recv() => {
                    let size = rect_from_size(terminal.size()?);
                    let mut should_draw_now =
                        !self.should_defer_net_event_redraw(&event, last_key_event.elapsed());
                    self.handle_net_event(event, size).await;
                    loop {
                        match self.rx.try_recv() {
                            Ok(next_event) => {
                                should_draw_now |= !self.should_defer_net_event_redraw(
                                    &next_event,
                                    last_key_event.elapsed(),
                                );
                                self.handle_net_event(next_event, size).await;
                            }
                            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                        }
                    }
                    if should_draw_now {
                        draw_requested = true;
                        pending_background_redraw = false;
                    } else {
                        pending_background_redraw = true;
                    }
                }
                _ = background_flush.tick(), if pending_background_redraw => {
                    if last_key_event.elapsed() >= INPUT_QUIET_WINDOW {
                        draw_requested = true;
                    }
                }
                _ = maintenance.tick() => {
                    let notes_changed = self.flush_notes_if_needed().await;
                    let draft_changed = self.flush_draft_if_needed().await;
                    if notes_changed || draft_changed {
                        if last_key_event.elapsed() < INPUT_QUIET_WINDOW {
                            pending_background_redraw = true;
                        } else {
                            draw_requested = true;
                        }
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
        if self.workspace_project_filter_picker.is_some() {
            self.handle_workspace_project_filter_picker_key(key, size);
            return;
        }
        if self.workspace_create_repo_picker.is_some() {
            self.handle_workspace_create_repo_picker_key(key);
            return;
        }
        if self.workspace_create_branch_picker.is_some() {
            self.handle_workspace_create_branch_picker_key(key);
            return;
        }
        if self.session_rename.is_some() {
            self.handle_session_rename_key(key).await;
            return;
        }
        if self.search_prompt.is_some() {
            self.handle_search_prompt_key(key, size).await;
            return;
        }

        if self.creating_workspace && key.code == crossterm::event::KeyCode::Esc {
            self.cancel_workspace_create_mode(size);
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

        if self.focus == Focus::Main
            && self.selected_pane == Pane::Notes
            && self.should_activate_notes_editor_from_main(key)
        {
            self.focus = Focus::Composer;
            self.handle_editor_key(key, true).await;
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

        if self.focus == Focus::Main && self.selected_pane == Pane::Chat {
            match key.code {
                crossterm::event::KeyCode::Char('[') => {
                    self.jump_to_user_message(false, self.page_step(size).max(1) as usize);
                    return;
                }
                crossterm::event::KeyCode::Char(']') => {
                    self.jump_to_user_message(true, self.page_step(size).max(1) as usize);
                    return;
                }
                crossterm::event::KeyCode::Char('n') if self.conversation_search.is_some() => {
                    self.advance_conversation_search(true, size);
                    return;
                }
                crossterm::event::KeyCode::Char('N') if self.conversation_search.is_some() => {
                    self.advance_conversation_search(false, size);
                    return;
                }
                _ => {}
            }
        }

        if self.creating_workspace
            && self.focus == Focus::Detail
            && self.selected_pane == Pane::Chat
        {
            match key.code {
                crossterm::event::KeyCode::Char('a') => {
                    self.open_workspace_create_repo_picker();
                    return;
                }
                crossterm::event::KeyCode::Enter => {
                    if let Some(repo) = self
                        .workspace_create_selected_repo()
                        .map(|entry| entry.repo.clone())
                    {
                        self.open_workspace_create_branch_picker(repo);
                    } else {
                        self.open_workspace_create_repo_picker();
                    }
                    return;
                }
                crossterm::event::KeyCode::Char('d') | crossterm::event::KeyCode::Backspace => {
                    self.remove_selected_workspace_create_repo();
                    return;
                }
                _ => {}
            }
        }

        if let Some(intent) = map_app_key(key, self.creating_new_session, &self.selected_pane) {
            self.handle_app_intent(intent, size).await;
        }
    }

    fn should_defer_net_event_redraw(
        &self,
        event: &NetEvent,
        time_since_last_key: Duration,
    ) -> bool {
        if time_since_last_key < Duration::from_millis(45) {
            return true;
        }

        if !self.is_input_sensitive_mode() {
            return false;
        }

        match event {
            NetEvent::UserSystemLoaded(_)
            | NetEvent::ActiveWorkspaces(_)
            | NetEvent::ArchivedWorkspaces(_)
            | NetEvent::Summaries(_)
            | NetEvent::WorkspaceLoaded(_)
            | NetEvent::SessionsLoaded { .. }
            | NetEvent::ReposLoaded { .. }
            | NetEvent::GitStatusLoaded { .. }
            | NetEvent::NotesLoaded { .. }
            | NetEvent::DiffsUpdated { .. }
            | NetEvent::ProcessesUpdated { .. }
            | NetEvent::LogsUpdated { .. }
            | NetEvent::ExecutorOptionsUpdated { .. }
            | NetEvent::ConversationHistoryLoaded { .. }
            | NetEvent::ConversationBootstrapComplete { .. }
            | NetEvent::ConversationBackfillComplete { .. }
            | NetEvent::WorkspaceCreateReposLoaded { .. }
            | NetEvent::WorkspaceCreateDraftLoaded { .. }
            | NetEvent::WorkspaceCreateDraftSaved { .. }
            | NetEvent::WorkspaceCreateDraftSaveFailed { .. }
            | NetEvent::WorkspaceCreateBranchesLoaded { .. }
            | NetEvent::WorkspaceCreateSubmitted { .. }
            | NetEvent::WorkspaceCreateSubmitFailed { .. }
            | NetEvent::DraftLoaded { .. }
            | NetEvent::QueueLoaded { .. } => true,
            NetEvent::TerminalConnected(_)
            | NetEvent::TerminalOutput(_, _)
            | NetEvent::TerminalError(_, _) => self.selected_pane != Pane::Terminal,
            NetEvent::DraftSaved { .. }
            | NetEvent::DraftSaveFailed { .. }
            | NetEvent::NotesSaved { .. }
            | NetEvent::NotesSaveFailed { .. }
            | NetEvent::PromptSubmitted { .. }
            | NetEvent::PromptSubmissionFailed { .. }
            | NetEvent::QueuedPrompt { .. }
            | NetEvent::QueuePromptFailed { .. }
            | NetEvent::QueueCancelled { .. }
            | NetEvent::QueueCancelFailed { .. }
            | NetEvent::DraftDiscarded { .. }
            | NetEvent::DraftDiscardFailed { .. }
            | NetEvent::WorkspaceActionFinished { .. }
            | NetEvent::Error(_) => false,
        }
    }

    fn is_input_sensitive_mode(&self) -> bool {
        self.focus == Focus::Composer
            || self.session_rename.is_some()
            || (self.bundle.terminal.input_mode && self.selected_pane == Pane::Terminal)
    }
}

fn should_process_key_event(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::should_process_key_event;

    #[test]
    fn ignores_release_events_but_keeps_press_and_repeat() {
        assert!(should_process_key_event(KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }));
        assert!(should_process_key_event(KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        }));
        assert!(!should_process_key_event(KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        }));
    }
}
