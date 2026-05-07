use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Text;

use crate::{
    app::{App, SessionRenameState},
    editor::{
        ComposerEditorMode, VimMode, VimOperator, apply_text_edit_action, clamp_char_boundary,
        line_end_index, line_start_index, move_cursor_vertical, next_char_boundary,
        next_word_start, prev_char_boundary, prev_word_start, render_editor_buffer,
    },
    input::{TextInputEvent, TextInputOptions, map_text_input_key},
    model::{Focus, Pane},
};

#[derive(Clone, Copy)]
enum EditorTarget {
    Composer,
    Notes,
}

impl App {
    pub(crate) async fn handle_text_input(&mut self, key: KeyEvent, notes: bool) {
        let (buffer, cursor): (&mut String, &mut usize) = if notes {
            (&mut self.bundle.notes, &mut self.notes_cursor)
        } else {
            (&mut self.composer, &mut self.composer_cursor)
        };
        let mut changed = false;
        let options = if notes {
            TextInputOptions {
                submit_on_enter: false,
                enter_inserts_newline: true,
                shift_enter_inserts_newline: false,
            }
        } else {
            TextInputOptions {
                submit_on_enter: true,
                enter_inserts_newline: false,
                shift_enter_inserts_newline: true,
            }
        };

        match map_text_input_key(key, options) {
            Some(TextInputEvent::Submit) => self.submit_prompt().await,
            Some(TextInputEvent::Edit(action)) => {
                let before = (buffer.clone(), *cursor);
                apply_text_edit_action(buffer, cursor, action);
                changed = before.0 != *buffer || before.1 != *cursor;
            }
            None => {}
        }
        if notes && changed {
            self.bundle.notes_dirty = true;
            self.bundle.last_notes_edit = Some(std::time::Instant::now());
            self.notes_edit_revision = self.notes_edit_revision.saturating_add(1);
        } else if changed {
            self.composer_dirty = true;
            self.last_composer_edit = Some(std::time::Instant::now());
            self.composer_edit_revision = self.composer_edit_revision.saturating_add(1);
        }
    }

    pub(crate) async fn handle_editor_key(&mut self, key: KeyEvent, notes: bool) {
        let target = if notes {
            EditorTarget::Notes
        } else {
            EditorTarget::Composer
        };

        if let KeyEvent {
            code: KeyCode::F(2),
            ..
        } = key
        {
            self.toggle_editor_mode();
            return;
        }

        match self.editor_mode {
            ComposerEditorMode::Standard => {
                if key.code == KeyCode::Esc {
                    if matches!(target, EditorTarget::Composer)
                        && self.creating_new_session
                        && self.composer.trim().is_empty()
                    {
                        self.cancel_new_session_flow();
                    } else {
                        self.focus = Focus::Main;
                    }
                } else {
                    self.handle_text_input(key, notes).await;
                }
            }
            ComposerEditorMode::Vim(VimMode::Insert) => {
                if key.code == KeyCode::Esc {
                    if matches!(target, EditorTarget::Composer)
                        && self.creating_new_session
                        && self.composer.trim().is_empty()
                    {
                        self.cancel_new_session_flow();
                    } else {
                        self.editor_mode = ComposerEditorMode::Vim(VimMode::Normal);
                        self.status = "Editor mode: Vim Normal".to_string();
                    }
                } else {
                    self.handle_text_input(key, notes).await;
                }
            }
            ComposerEditorMode::Vim(VimMode::Normal) => {
                if self.handle_vim_normal_key(key, target).await {
                    return;
                }
                if key.code == KeyCode::Esc {
                    if matches!(target, EditorTarget::Composer)
                        && self.creating_new_session
                        && self.composer.trim().is_empty()
                    {
                        self.cancel_new_session_flow();
                    } else {
                        self.focus = Focus::Main;
                    }
                }
            }
        }
    }

    async fn handle_vim_normal_key(&mut self, key: KeyEvent, target: EditorTarget) -> bool {
        if let Some(operator) = self.vim_pending_operator.take() {
            return self.execute_vim_operator(operator, key, target);
        }

        match key {
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                if matches!(target, EditorTarget::Composer) {
                    self.submit_prompt().await;
                }
                true
            }
            KeyEvent {
                code: KeyCode::Char('i'),
                ..
            } => {
                self.editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Editor mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('a'),
                ..
            } => {
                {
                    let (buffer, cursor): (&mut String, &mut usize) =
                        self.editor_buffer_cursor_mut(target);
                    *cursor = next_char_boundary(buffer, *cursor);
                }
                self.editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Editor mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('I'),
                ..
            } => {
                {
                    let (buffer, cursor): (&mut String, &mut usize) =
                        self.editor_buffer_cursor_mut(target);
                    *cursor = line_start_index(buffer, *cursor);
                }
                self.editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Editor mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('A'),
                ..
            } => {
                {
                    let (buffer, cursor): (&mut String, &mut usize) =
                        self.editor_buffer_cursor_mut(target);
                    *cursor = line_end_index(buffer, *cursor);
                }
                self.editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Editor mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('h') | KeyCode::Left,
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = prev_char_boundary(buffer, *cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('l') | KeyCode::Right,
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = next_char_boundary(buffer, *cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = move_cursor_vertical(buffer, *cursor, -1);
                true
            }
            KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = move_cursor_vertical(buffer, *cursor, 1);
                true
            }
            KeyEvent {
                code: KeyCode::Char('w'),
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = next_word_start(buffer, *cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('b'),
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = prev_word_start(buffer, *cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('d'),
                ..
            } => {
                self.vim_pending_operator = Some(VimOperator::Delete);
                self.status = "d...".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('0') | KeyCode::Home,
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = line_start_index(buffer, *cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('$') | KeyCode::End,
                ..
            } => {
                let (buffer, cursor): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                *cursor = line_end_index(buffer, *cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('x') | KeyCode::Delete,
                ..
            } => {
                let changed = {
                    let (buffer, cursor): (&mut String, &mut usize) =
                        self.editor_buffer_cursor_mut(target);
                    *cursor = clamp_char_boundary(buffer, *cursor);
                    if *cursor < buffer.len() {
                        let end = next_char_boundary(buffer, *cursor);
                        buffer.drain(*cursor..end);
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    self.mark_editor_dirty(target);
                }
                true
            }
            KeyEvent {
                code: KeyCode::Char('o'),
                ..
            } => {
                {
                    let (buffer, cursor): (&mut String, &mut usize) =
                        self.editor_buffer_cursor_mut(target);
                    *cursor = clamp_char_boundary(buffer, *cursor);
                    *cursor = line_end_index(buffer, *cursor);
                    buffer.insert(*cursor, '\n');
                    *cursor += 1;
                }
                self.mark_editor_dirty(target);
                self.editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Editor mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('O'),
                ..
            } => {
                {
                    let (buffer, cursor): (&mut String, &mut usize) =
                        self.editor_buffer_cursor_mut(target);
                    *cursor = clamp_char_boundary(buffer, *cursor);
                    *cursor = line_start_index(buffer, *cursor);
                    buffer.insert(*cursor, '\n');
                }
                self.mark_editor_dirty(target);
                self.editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Editor mode: Vim Insert".to_string();
                true
            }
            _ => false,
        }
    }

    fn execute_vim_operator(
        &mut self,
        operator: VimOperator,
        key: KeyEvent,
        target: EditorTarget,
    ) -> bool {
        match operator {
            VimOperator::Delete => self.execute_vim_delete(key, target),
        }
    }

    fn execute_vim_delete(&mut self, key: KeyEvent, target: EditorTarget) -> bool {
        let range = {
            let (buffer, cursor): (&mut String, &mut usize) = self.editor_buffer_cursor_mut(target);
            let cursor_value = clamp_char_boundary(buffer, *cursor);
            match key {
                KeyEvent {
                    code: KeyCode::Char('d'),
                    ..
                } => {
                    let line_start = line_start_index(buffer, cursor_value);
                    let line_end = line_end_index(buffer, cursor_value);
                    if line_end < buffer.len() {
                        Some((line_start, line_end + 1))
                    } else if line_start > 0 {
                        Some((line_start - 1, line_end))
                    } else {
                        Some((0, line_end))
                    }
                }
                KeyEvent {
                    code: KeyCode::Char('w'),
                    ..
                } => Some((cursor_value, next_word_start(buffer, cursor_value))),
                KeyEvent {
                    code: KeyCode::Char('b'),
                    ..
                } => {
                    let delete_start = prev_word_start(buffer, cursor_value);
                    Some((delete_start, cursor_value))
                }
                KeyEvent {
                    code: KeyCode::Char('$') | KeyCode::End,
                    ..
                } => Some((cursor_value, line_end_index(buffer, cursor_value))),
                KeyEvent {
                    code: KeyCode::Char('0') | KeyCode::Home,
                    ..
                } => Some((line_start_index(buffer, cursor_value), cursor_value)),
                _ => None,
            }
        };

        if let Some((start, end)) = range
            && start < end
        {
            let changed = {
                let (buffer, cursor_ref): (&mut String, &mut usize) =
                    self.editor_buffer_cursor_mut(target);
                if end <= buffer.len()
                    && buffer.is_char_boundary(start)
                    && buffer.is_char_boundary(end)
                {
                    buffer.drain(start..end);
                    *cursor_ref = start.min(buffer.len());
                    true
                } else {
                    false
                }
            };
            if changed {
                self.mark_editor_dirty(target);
            }
        }
        self.status = "Editor mode: Vim Normal".to_string();
        true
    }

    fn editor_buffer_cursor_mut(&mut self, target: EditorTarget) -> (&mut String, &mut usize) {
        match target {
            EditorTarget::Composer => (&mut self.composer, &mut self.composer_cursor),
            EditorTarget::Notes => (&mut self.bundle.notes, &mut self.notes_cursor),
        }
    }

    fn mark_editor_dirty(&mut self, target: EditorTarget) {
        match target {
            EditorTarget::Composer => {
                self.composer_dirty = true;
                self.last_composer_edit = Some(std::time::Instant::now());
                self.composer_edit_revision = self.composer_edit_revision.saturating_add(1);
            }
            EditorTarget::Notes => {
                self.bundle.notes_dirty = true;
                self.bundle.last_notes_edit = Some(std::time::Instant::now());
                self.notes_edit_revision = self.notes_edit_revision.saturating_add(1);
            }
        }
    }

    pub(crate) fn editor_mode_label(&self) -> &'static str {
        match self.editor_mode {
            ComposerEditorMode::Standard => "standard",
            ComposerEditorMode::Vim(VimMode::Insert) => "vim insert",
            ComposerEditorMode::Vim(VimMode::Normal) => "vim normal",
        }
    }

    pub(crate) fn toggle_editor_mode(&mut self) {
        self.editor_mode = match self.editor_mode {
            ComposerEditorMode::Standard => ComposerEditorMode::Vim(VimMode::Insert),
            ComposerEditorMode::Vim(_) => ComposerEditorMode::Standard,
        };
        self.status = format!("Editor mode: {}", self.editor_mode_label());
    }

    pub(crate) fn editor_panel_title(&self) -> String {
        let label = match self.selected_pane {
            Pane::Notes => "Notes Editor",
            _ => "Composer",
        };
        format!("{label} [{}]", self.editor_mode_label())
    }

    pub(crate) fn render_composer_text(&self) -> Text<'static> {
        let show_cursor = self.focus == Focus::Composer && self.selected_pane == Pane::Chat;
        render_editor_buffer(
            &self.composer,
            clamp_char_boundary(&self.composer, self.composer_cursor),
            show_cursor,
            self.editor_mode,
        )
    }

    pub(crate) fn open_session_rename(&mut self) {
        if self.focus != Focus::Detail || !matches!(self.selected_pane, Pane::Chat | Pane::Logs) {
            return;
        }
        let Some(session) = self.selected_session_for_rename() else {
            self.status = "Select a real session to rename".to_string();
            return;
        };
        let name = session
            .name
            .clone()
            .unwrap_or_else(|| session.id.to_string());
        self.session_rename = Some(SessionRenameState {
            session_id: session.id,
            cursor: name.len(),
            name,
        });
        self.status = "Rename session".to_string();
        self.error = None;
    }

    pub(crate) async fn handle_session_rename_key(&mut self, key: KeyEvent) {
        let Some(rename) = self.session_rename.as_mut() else {
            return;
        };

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.session_rename = None;
                self.status = "Cancelled session rename".to_string();
            }
            _ => match map_text_input_key(
                key,
                TextInputOptions {
                    submit_on_enter: true,
                    enter_inserts_newline: false,
                    shift_enter_inserts_newline: false,
                },
            ) {
                Some(TextInputEvent::Submit) => self.submit_session_rename().await,
                Some(TextInputEvent::Edit(action)) => {
                    apply_text_edit_action(&mut rename.name, &mut rename.cursor, action);
                    rename.cursor = clamp_char_boundary(&rename.name, rename.cursor);
                }
                None => {}
            },
        }
    }

    async fn submit_session_rename(&mut self) {
        let Some(rename) = self.session_rename.take() else {
            return;
        };
        let trimmed = rename.name.trim().to_string();
        if trimmed.is_empty() {
            self.error = Some("Session name cannot be empty".to_string());
            self.status = "Session name cannot be empty".to_string();
            self.session_rename = Some(rename);
            return;
        }

        match self
            .api
            .rename_session(rename.session_id, trimmed.clone())
            .await
        {
            Ok(updated) => {
                if let Some(session) = self
                    .bundle
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == updated.id)
                {
                    *session = updated;
                }
                self.status = format!("Renamed session to {trimmed}");
                self.error = None;
            }
            Err(error) => {
                let cursor = clamp_char_boundary(&rename.name, rename.cursor);
                self.session_rename = Some(SessionRenameState {
                    session_id: rename.session_id,
                    cursor,
                    name: rename.name,
                });
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }
}
