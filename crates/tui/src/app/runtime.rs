use std::time::Duration;

use db::models::{execution_process::ExecutionProcessStatus, scratch::DraftFollowUpData};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::{
    api::TerminalCommand,
    app::App,
    conversation::render_log_entry,
    model::{Focus, NetEvent, Pane},
    ui::terminal_content_area,
    workspace::{WorkspaceRow, session_target},
};

impl App {
    pub(crate) async fn submit_prompt(&mut self) {
        if self.creating_workspace {
            self.submit_workspace_create().await;
            return;
        }
        let Some(workspace_id) = self.selected_workspace_id else {
            self.status = "No workspace selected".to_string();
            return;
        };
        if self.actions_in_flight.prompt_submit {
            self.status = "Prompt submission already in progress".to_string();
            return;
        }
        let prompt = self.expanded_composer().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if self.has_running_process() && !self.creating_new_session {
            self.queue_prompt().await;
            return;
        }
        let Some(executor_config) = self.composer_config.clone() else {
            self.status = "Composer config is still loading".to_string();
            return;
        };
        let restored_draft = DraftFollowUpData {
            message: self.serialized_composer_for_draft(),
            executor_config: executor_config.clone(),
        };
        let scratch_id = self.current_composer_scratch_id();
        let optimistic_scope = self.current_conversation_scope();
        let session = if self.creating_new_session {
            None
        } else {
            self.current_session().cloned()
        };
        self.composer.clear();
        self.composer_snippets.clear();
        self.invalidate_composer_layout_cache();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.last_composer_edit = None;
        self.composer_queue_conflict = false;
        self.focus = Focus::Main;
        let optimistic_id = optimistic_scope.clone().map(|scope| {
            self.push_optimistic_entry(scope, prompt.clone(), executor_config.clone())
        });
        self.actions_in_flight.prompt_submit = true;
        self.status = "Sending prompt".to_string();
        let api = self.api.clone();
        let tx = self.tx.clone();
        let workspace_scope = self
            .selected_workspace_id
            .filter(|_| self.creating_new_session);
        tokio::spawn(async move {
            match api
                .send_prompt(workspace_id, session, prompt, executor_config)
                .await
            {
                Ok(session_id) => {
                    if let Some(scratch_id) = scratch_id {
                        let _ = api.delete_follow_up_draft(scratch_id).await;
                    }
                    let _ = tx.send(NetEvent::PromptSubmitted {
                        workspace_id,
                        session_id,
                        workspace_scope,
                    });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::PromptSubmissionFailed {
                        message: error.to_string(),
                        restored_draft,
                        optimistic_id,
                    });
                }
            }
        });
    }

    pub(crate) async fn flush_notes_if_needed(&mut self) -> bool {
        let Some(workspace_id) = self.selected_workspace_id else {
            return false;
        };
        if !self.bundle.notes_dirty || self.notes_save_in_flight {
            return false;
        }
        let Some(last_edit) = self.bundle.last_notes_edit else {
            return false;
        };
        if last_edit.elapsed() < Duration::from_millis(900) {
            return false;
        }
        self.notes_save_in_flight = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        let notes = self.bundle.notes.clone();
        let revision = self.notes_edit_revision;
        tokio::spawn(async move {
            match api.save_notes(workspace_id, notes).await {
                Ok(()) => {
                    let _ = tx.send(NetEvent::NotesSaved {
                        workspace_id,
                        revision,
                    });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::NotesSaveFailed {
                        workspace_id,
                        revision,
                        message: error.to_string(),
                    });
                }
            }
        });
        true
    }

    pub(crate) fn handle_terminal_resize(&mut self, size: Rect) {
        let cols = size.width.max(1);
        let rows = size.height.max(1);
        if self.bundle.terminal.size != (cols, rows) {
            self.bundle.terminal.size = (cols, rows);
            self.bundle.terminal.parser.set_size(rows, cols);
            self.mark_terminal_dirty();
            if let Some(tx) = &self.subscriptions.terminal_tx {
                let _ = tx.send(TerminalCommand::Resize(cols, rows));
            }
        }
    }

    pub(crate) async fn handle_enter(&mut self, size: Rect) {
        match self.focus {
            Focus::WorkspaceList => {
                let rows = self.workspace_rows();
                let selected_index = if self.creating_workspace {
                    rows.iter()
                        .position(|row| matches!(row, WorkspaceRow::NewWorkspace))
                        .unwrap_or(0)
                } else {
                    self.selected_workspace_id
                        .and_then(|selected_id| {
                            rows.iter().position(|row| match row {
                                WorkspaceRow::NewWorkspace | WorkspaceRow::Header(_) => false,
                                WorkspaceRow::Workspace(workspace) => workspace.id == selected_id,
                            })
                        })
                        .unwrap_or(0)
                };
                let target = rows.get(selected_index).map(|row| match row {
                    WorkspaceRow::NewWorkspace => None,
                    WorkspaceRow::Header(_) => None,
                    WorkspaceRow::Workspace(workspace) => Some(workspace.id),
                });
                match target {
                    Some(None) => self.enter_workspace_create_mode(),
                    Some(Some(workspace_id)) => {
                        self.creating_workspace = false;
                        self.selected_workspace_id = Some(workspace_id);
                        self.ensure_workspace_selected(size);
                    }
                    _ => {}
                }
            }
            Focus::Main => {
                if self.selected_pane == Pane::Terminal {
                    self.bundle.terminal.input_mode = true;
                    self.status = "Terminal input mode enabled".to_string();
                }
            }
            Focus::Detail => self.switch_session_or_process(size),
            Focus::Composer => {}
        }
    }

    pub(crate) fn move_selection(&mut self, delta: i32, size: Rect) {
        match self.focus {
            Focus::WorkspaceList => {
                let rows = self.workspace_rows();
                let selectable = rows
                    .iter()
                    .enumerate()
                    .filter_map(|(index, row)| match row {
                        WorkspaceRow::NewWorkspace | WorkspaceRow::Workspace(_) => Some(index),
                        WorkspaceRow::Header(_) => None,
                    })
                    .collect::<Vec<_>>();
                if selectable.is_empty() {
                    return;
                }
                let current_row_index = if self.creating_workspace {
                    rows.iter()
                        .position(|row| matches!(row, WorkspaceRow::NewWorkspace))
                        .unwrap_or(selectable[0])
                } else {
                    self.selected_workspace_id
                        .and_then(|id| {
                            rows.iter().position(|row| match row {
                                WorkspaceRow::NewWorkspace | WorkspaceRow::Header(_) => false,
                                WorkspaceRow::Workspace(workspace) => workspace.id == id,
                            })
                        })
                        .unwrap_or(selectable[0])
                };
                let current_index = selectable
                    .iter()
                    .position(|index| *index == current_row_index)
                    .unwrap_or(0) as i32;
                let next_index = (current_index + delta)
                    .clamp(0, selectable.len().saturating_sub(1) as i32)
                    as usize;
                let next_row = rows.get(selectable[next_index]).map(|row| match row {
                    WorkspaceRow::NewWorkspace => None,
                    WorkspaceRow::Header(_) => None,
                    WorkspaceRow::Workspace(workspace) => Some(workspace.id),
                });
                match next_row {
                    Some(None) => {
                        self.enter_workspace_create_mode();
                    }
                    Some(Some(workspace_id)) => {
                        self.creating_workspace = false;
                        self.selected_workspace_id = Some(workspace_id);
                        self.mark_workspace_list_dirty();
                        self.load_selected_workspace(size);
                    }
                    _ => {}
                }
            }
            Focus::Detail => match self.selected_pane {
                Pane::Chat if self.creating_workspace => {
                    self.move_workspace_create_selection(delta);
                    self.mark_detail_dirty();
                }
                Pane::Chat | Pane::Logs => {
                    let rows = self.session_rows();
                    if rows.is_empty() {
                        return;
                    }
                    let current = self.selected_session_row_index(&rows).unwrap_or(0) as i32;
                    let next =
                        (current + delta).clamp(0, rows.len().saturating_sub(1) as i32) as usize;
                    if let Some(target) = session_target(&rows[next]) {
                        self.select_session_target(target);
                    }
                }
                Pane::Git => {
                    let scroll = self.bundle.log_scroll as i32 + delta;
                    self.bundle.log_scroll = scroll.clamp(0, u16::MAX as i32) as u16;
                }
                _ => {}
            },
            Focus::Main | Focus::Composer => match self.selected_pane {
                Pane::Changes => {
                    if self.bundle.diffs.is_empty() {
                        return;
                    }
                    let next = (self.bundle.selected_diff_index as i32 + delta)
                        .clamp(0, self.bundle.diffs.len().saturating_sub(1) as i32)
                        as usize;
                    if self.bundle.selected_diff_index != next {
                        self.bundle.selected_diff_index = next;
                        self.mark_changes_dirty();
                    }
                }
                Pane::Chat => self.adjust_chat_scroll(delta),
                Pane::Logs => {
                    let scroll = self.bundle.log_scroll as i32 + delta;
                    self.bundle.log_scroll = scroll.clamp(0, u16::MAX as i32) as u16;
                }
                _ => {}
            },
        }
    }

    pub(crate) fn page_step(&self, size: Rect) -> i32 {
        match self.focus {
            Focus::WorkspaceList => ((size.height.saturating_sub(4) / 3).max(1)) as i32,
            Focus::Detail => 5,
            Focus::Main | Focus::Composer => size.height.saturating_sub(6).max(1) as i32,
        }
    }

    pub(crate) fn jump_to_boundary(&mut self, to_end: bool, size: Rect) {
        match self.focus {
            Focus::WorkspaceList => {
                let rows = self.workspace_rows();
                let selectable = rows
                    .iter()
                    .enumerate()
                    .filter_map(|(index, row)| match row {
                        WorkspaceRow::NewWorkspace | WorkspaceRow::Workspace(_) => Some(index),
                        WorkspaceRow::Header(_) => None,
                    })
                    .collect::<Vec<_>>();
                if selectable.is_empty() {
                    return;
                }
                let row_index = if to_end {
                    *selectable.last().unwrap_or(&selectable[0])
                } else {
                    selectable[0]
                };
                let row_target = rows.get(row_index).map(|row| match row {
                    WorkspaceRow::NewWorkspace => None,
                    WorkspaceRow::Header(_) => None,
                    WorkspaceRow::Workspace(workspace) => Some(workspace.id),
                });
                match row_target {
                    Some(None) => self.enter_workspace_create_mode(),
                    Some(Some(workspace_id)) => {
                        self.creating_workspace = false;
                        self.selected_workspace_id = Some(workspace_id);
                        self.mark_workspace_list_dirty();
                        self.load_selected_workspace(size);
                    }
                    _ => {}
                }
            }
            Focus::Detail => match self.selected_pane {
                Pane::Chat if self.creating_workspace => {
                    if let Some(state) = self.workspace_create.as_mut()
                        && !state.selected_repos.is_empty()
                    {
                        state.selected_repo_index = if to_end {
                            state.selected_repos.len().saturating_sub(1)
                        } else {
                            0
                        };
                        self.mark_detail_dirty();
                    }
                }
                Pane::Chat | Pane::Logs => {
                    let rows = self.session_rows();
                    if rows.is_empty() {
                        return;
                    }
                    let target = if to_end {
                        rows.last().unwrap_or(&rows[0])
                    } else {
                        &rows[0]
                    };
                    if let Some(target) = session_target(target) {
                        self.select_session_target(target);
                    }
                }
                Pane::Git => {
                    self.bundle.log_scroll = if to_end {
                        self.max_scroll_for_selected_pane()
                    } else {
                        0
                    };
                }
                _ => {}
            },
            Focus::Main | Focus::Composer => match self.selected_pane {
                Pane::Changes => {
                    if self.bundle.diffs.is_empty() {
                        return;
                    }
                    let next = if to_end {
                        self.bundle.diffs.len().saturating_sub(1)
                    } else {
                        0
                    };
                    if self.bundle.selected_diff_index != next {
                        self.bundle.selected_diff_index = next;
                        self.mark_changes_dirty();
                    }
                }
                Pane::Chat => self.chat_end_offset = if to_end { 0 } else { u16::MAX },
                Pane::Logs | Pane::Git => {
                    self.bundle.log_scroll = if to_end {
                        self.max_scroll_for_selected_pane()
                    } else {
                        0
                    };
                }
                _ => {}
            },
        }
    }

    fn max_scroll_for_selected_pane(&self) -> u16 {
        let lines = match self.selected_pane {
            Pane::Chat => self.chat_line_count(),
            Pane::Logs => self
                .bundle
                .log_entries
                .iter()
                .enumerate()
                .map(|(index, entry)| render_log_entry(index, entry).len())
                .sum::<usize>(),
            Pane::Git => self
                .bundle
                .git_status
                .iter()
                .map(|status| 3 + usize::from(status.status.is_rebase_in_progress))
                .sum::<usize>(),
            _ => 0,
        };
        lines.saturating_sub(1).min(u16::MAX as usize) as u16
    }

    pub(crate) fn terminal_stream_size(&self, size: Rect) -> (u16, u16) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(size);
        let body = if size.width >= 140 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(32),
                    Constraint::Min(50),
                    Constraint::Length(44),
                ])
                .split(outer[1])[1]
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(32), Constraint::Min(40)])
                .split(outer[1])[1]
        };
        let main = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(5),
            ])
            .split(body)[1];
        let content = terminal_content_area(main);
        (content.width.max(1), content.height.max(1))
    }

    fn adjust_chat_scroll(&mut self, delta: i32) {
        let offset = self.chat_end_offset as i32 - delta;
        self.chat_end_offset = offset.clamp(0, u16::MAX as i32) as u16;
    }

    pub(crate) fn has_running_process(&self) -> bool {
        self.bundle
            .process_map
            .values()
            .any(|process| process.status == ExecutionProcessStatus::Running)
    }

    pub(crate) fn send_terminal_input(&mut self, bytes: Vec<u8>) {
        let Some(tx) = &self.subscriptions.terminal_tx else {
            return;
        };
        let _ = tx.send(TerminalCommand::Input(bytes));
    }
}
