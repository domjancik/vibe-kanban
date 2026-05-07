use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
    SessionRename,
}

impl App {
    fn notes_text_input_options() -> TextInputOptions {
        TextInputOptions {
            submit_on_enter: false,
            enter_inserts_newline: true,
            shift_enter_inserts_newline: false,
        }
    }

    async fn handle_target_text_input(&mut self, key: KeyEvent, target: EditorTarget) {
        let options = match target {
            EditorTarget::Composer => TextInputOptions {
                submit_on_enter: true,
                enter_inserts_newline: false,
                shift_enter_inserts_newline: true,
            },
            EditorTarget::Notes => Self::notes_text_input_options(),
            EditorTarget::SessionRename => TextInputOptions {
                submit_on_enter: true,
                enter_inserts_newline: false,
                shift_enter_inserts_newline: false,
            },
        };

        let event = map_text_input_key(key, options);
        match event {
            Some(TextInputEvent::Submit) => match target {
                EditorTarget::Composer => self.submit_prompt().await,
                EditorTarget::SessionRename => self.submit_session_rename().await,
                EditorTarget::Notes => {}
            },
            Some(TextInputEvent::Edit(action)) => {
                let changed = {
                    let (buffer, cursor): (&mut String, &mut usize) =
                        self.editor_buffer_cursor_mut(target);
                    apply_text_edit_action(buffer, cursor, action)
                };
                if changed {
                    self.mark_editor_dirty(target);
                }
            }
            None => {}
        }
    }

    pub(crate) fn should_activate_notes_editor_from_main(&self, key: KeyEvent) -> bool {
        if matches!(key.code, KeyCode::F(2)) {
            return true;
        }
        if matches!(key.code, KeyCode::Char('1'..='6'))
            && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
        {
            return false;
        }

        match self.editor_mode {
            ComposerEditorMode::Standard | ComposerEditorMode::Vim(VimMode::Insert) => {
                map_text_input_key(key, Self::notes_text_input_options()).is_some()
            }
            ComposerEditorMode::Vim(VimMode::Normal) => matches!(
                key.code,
                KeyCode::Enter
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Char('i')
                    | KeyCode::Char('a')
                    | KeyCode::Char('I')
                    | KeyCode::Char('A')
                    | KeyCode::Char('h')
                    | KeyCode::Char('j')
                    | KeyCode::Char('k')
                    | KeyCode::Char('l')
                    | KeyCode::Char('o')
                    | KeyCode::Char('O')
                    | KeyCode::Char('x')
                    | KeyCode::Char('d')
                    | KeyCode::Char('0')
                    | KeyCode::Char('$')
            ),
        }
    }

    pub(crate) async fn handle_text_input(&mut self, key: KeyEvent, notes: bool) {
        let target = if notes {
            EditorTarget::Notes
        } else {
            EditorTarget::Composer
        };
        self.handle_target_text_input(key, target).await;
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
            EditorTarget::SessionRename => {
                let rename = self
                    .session_rename
                    .as_mut()
                    .expect("session rename target requires active state");
                (&mut rename.name, &mut rename.cursor)
            }
        }
    }

    fn mark_editor_dirty(&mut self, target: EditorTarget) {
        match target {
            EditorTarget::Composer => {
                self.invalidate_composer_layout_cache();
                self.composer_dirty = true;
                self.last_composer_edit = Some(std::time::Instant::now());
                self.composer_edit_revision = self.composer_edit_revision.saturating_add(1);
            }
            EditorTarget::Notes => {
                self.bundle.notes_dirty = true;
                self.bundle.last_notes_edit = Some(std::time::Instant::now());
                self.notes_edit_revision = self.notes_edit_revision.saturating_add(1);
            }
            EditorTarget::SessionRename => self.mark_detail_dirty(),
        }
    }

    pub(crate) fn invalidate_composer_layout_cache(&mut self) {
        self.composer_height_cache = None;
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
        self.mark_detail_dirty();
        self.status = "Rename session".to_string();
        self.error = None;
    }

    pub(crate) async fn handle_session_rename_key(&mut self, key: KeyEvent) {
        if self.session_rename.is_none() {
            return;
        }

        if let KeyEvent {
            code: KeyCode::F(2),
            ..
        } = key
        {
            self.toggle_editor_mode();
            return;
        }

        match self.editor_mode {
            ComposerEditorMode::Standard => match key {
                KeyEvent {
                    code: KeyCode::Esc, ..
                } => {
                    self.session_rename = None;
                    self.mark_detail_dirty();
                    self.status = "Cancelled session rename".to_string();
                }
                _ => {
                    self.handle_target_text_input(key, EditorTarget::SessionRename)
                        .await
                }
            },
            ComposerEditorMode::Vim(VimMode::Insert) => {
                if key.code == KeyCode::Esc {
                    self.editor_mode = ComposerEditorMode::Vim(VimMode::Normal);
                    self.status = "Editor mode: Vim Normal".to_string();
                } else {
                    self.handle_target_text_input(key, EditorTarget::SessionRename)
                        .await;
                }
            }
            ComposerEditorMode::Vim(VimMode::Normal) => {
                if self
                    .handle_vim_normal_key(key, EditorTarget::SessionRename)
                    .await
                {
                    return;
                }
                if key.code == KeyCode::Esc {
                    self.session_rename = None;
                    self.mark_detail_dirty();
                    self.status = "Cancelled session rename".to_string();
                }
            }
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
                self.mark_detail_dirty();
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
                self.mark_detail_dirty();
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent};
    use db::models::session::Session;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc::unbounded_channel,
    };
    use uuid::Uuid;

    use super::EditorTarget;
    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::{App, SessionRenameState},
        editor::{ComposerEditorMode, VimMode},
        model::{Focus, Pane, QueueStatus, WorkspaceBundle},
    };

    fn test_app() -> App {
        let api = Api::new("http://127.0.0.1:9".to_string()).unwrap();
        let (tx, rx) = unbounded_channel();
        App {
            api,
            rx,
            tx,
            workspace_streams: Vec::new(),
            summary_streams: Vec::new(),
            subscriptions: WorkspaceSubscriptions::default(),
            active_workspaces: HashMap::new(),
            archived_workspaces: HashMap::new(),
            summaries: HashMap::new(),
            selected_workspace_id: None,
            selected_pane: Pane::Chat,
            focus: Focus::Main,
            maximized_panel: false,
            show_archived: false,
            filter: String::new(),
            session_filter: String::new(),
            workspace_list_revision: 0,
            detail_revision: 0,
            workspace_list_cache: None,
            detail_pane_cache: None,
            status: String::new(),
            error: None,
            bundle: WorkspaceBundle::default(),
            executor_profiles: executors::profile::ExecutorConfigs {
                executors: HashMap::new(),
            },
            default_executor_profile: None,
            composer_config: None,
            composer_options: None,
            composer: String::new(),
            composer_cursor: 0,
            editor_mode: ComposerEditorMode::Standard,
            vim_pending_operator: None,
            composer_dirty: false,
            composer_edit_revision: 0,
            composer_height_cache: None,
            draft_save_in_flight: false,
            composer_queue_conflict: false,
            composer_scratch_id: None,
            composer_scratch_loaded: false,
            queue_session_id: None,
            queue_status: QueueStatus::Empty,
            queue_pending: false,
            last_composer_edit: None,
            chat_end_offset: 0,
            chat_render_cache: None,
            chat_render_cache_dirty: true,
            last_chat_render_cache_build: None,
            conversation_loader: None,
            conversation_process_entries: HashMap::new(),
            conversation_process_order: Vec::new(),
            conversation_bootstrapping: false,
            conversation_backfilling: false,
            optimistic_entries: Vec::new(),
            notes_cursor: 0,
            notes_edit_revision: 0,
            notes_save_in_flight: false,
            agent_picker: None,
            session_rename: None,
            search_prompt: None,
            conversation_search: None,
            tool_call_display_mode: crate::app::ToolCallDisplayMode::Expanded,
            actions_in_flight: Default::default(),
            creating_new_session: false,
            should_quit: false,
        }
    }

    fn session(id: Uuid, name: &str) -> Session {
        let now = Utc.timestamp_opt(1, 0).unwrap();
        Session {
            id,
            workspace_id: Uuid::new_v4(),
            name: Some(name.to_string()),
            executor: Some("CODEX".to_string()),
            agent_working_dir: None,
            created_at: now,
            updated_at: now,
        }
    }

    async fn spawn_rename_server(session: Session) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 4096];
            let _ = socket.read(&mut buffer).await.unwrap();
            let body = serde_json::json!({
                "success": true,
                "data": session,
                "message": null
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{}", addr)
    }

    #[test]
    fn editor_buffer_cursor_mut_routes_targets_correctly() {
        let mut app = test_app();
        app.composer = "composer".to_string();
        app.composer_cursor = 3;
        app.bundle.notes = "notes".to_string();
        app.notes_cursor = 2;
        app.session_rename = Some(SessionRenameState {
            session_id: Uuid::new_v4(),
            name: "rename".to_string(),
            cursor: 4,
        });

        let (buffer, cursor) = app.editor_buffer_cursor_mut(EditorTarget::Composer);
        assert_eq!(buffer.as_str(), "composer");
        assert_eq!(*cursor, 3);

        let (buffer, cursor) = app.editor_buffer_cursor_mut(EditorTarget::Notes);
        assert_eq!(buffer.as_str(), "notes");
        assert_eq!(*cursor, 2);

        let (buffer, cursor) = app.editor_buffer_cursor_mut(EditorTarget::SessionRename);
        assert_eq!(buffer.as_str(), "rename");
        assert_eq!(*cursor, 4);
    }

    #[test]
    fn mark_editor_dirty_updates_only_persisted_editor_targets() {
        let mut app = test_app();
        app.session_rename = Some(SessionRenameState {
            session_id: Uuid::new_v4(),
            name: "rename".to_string(),
            cursor: 6,
        });

        app.mark_editor_dirty(EditorTarget::Composer);
        assert!(app.composer_dirty);
        assert_eq!(app.composer_edit_revision, 1);

        app.mark_editor_dirty(EditorTarget::Notes);
        assert!(app.bundle.notes_dirty);
        assert_eq!(app.notes_edit_revision, 1);

        app.mark_editor_dirty(EditorTarget::SessionRename);
        assert_eq!(app.composer_edit_revision, 1);
        assert_eq!(app.notes_edit_revision, 1);
    }

    #[tokio::test]
    async fn session_rename_uses_shared_vim_mode_rules() {
        let mut app = test_app();
        app.session_rename = Some(SessionRenameState {
            session_id: Uuid::new_v4(),
            name: "name".to_string(),
            cursor: 4,
        });
        app.editor_mode = ComposerEditorMode::Vim(VimMode::Insert);

        app.handle_session_rename_key(KeyEvent::from(KeyCode::Esc))
            .await;
        assert!(app.session_rename.is_some());
        assert!(matches!(
            app.editor_mode,
            ComposerEditorMode::Vim(VimMode::Normal)
        ));

        app.handle_session_rename_key(KeyEvent::from(KeyCode::Char('h')))
            .await;
        assert_eq!(
            app.session_rename.as_ref().map(|rename| rename.cursor),
            Some(3)
        );

        app.handle_session_rename_key(KeyEvent::from(KeyCode::Esc))
            .await;
        assert!(app.session_rename.is_none());
    }

    #[tokio::test]
    async fn session_rename_standard_mode_escape_cancels_and_enter_saves() {
        let session_id = Uuid::new_v4();
        let updated = session(session_id, "renamed");
        let mut app = test_app();
        app.api = Api::new(spawn_rename_server(updated.clone()).await).unwrap();
        app.bundle.sessions = vec![session(session_id, "old")];
        app.session_rename = Some(SessionRenameState {
            session_id,
            name: "renamed".to_string(),
            cursor: 7,
        });

        app.handle_session_rename_key(KeyEvent::from(KeyCode::Enter))
            .await;
        assert!(app.session_rename.is_none());
        assert_eq!(app.bundle.sessions[0].name.as_deref(), Some("renamed"));

        app.session_rename = Some(SessionRenameState {
            session_id,
            name: "temp".to_string(),
            cursor: 4,
        });
        app.handle_session_rename_key(KeyEvent::from(KeyCode::Esc))
            .await;
        assert!(app.session_rename.is_none());
        assert_eq!(app.status, "Cancelled session rename");
    }

    #[test]
    fn vim_delete_operator_applies_across_editor_targets() {
        let mut app = test_app();
        app.composer = "hello world".to_string();
        app.composer_cursor = 0;
        app.bundle.notes = "note words".to_string();
        app.notes_cursor = 5;
        app.session_rename = Some(SessionRenameState {
            session_id: Uuid::new_v4(),
            name: "rename me".to_string(),
            cursor: 7,
        });

        app.execute_vim_delete(KeyEvent::from(KeyCode::Char('w')), EditorTarget::Composer);
        assert_eq!(app.composer, "world");

        app.execute_vim_delete(KeyEvent::from(KeyCode::Char('b')), EditorTarget::Notes);
        assert_eq!(app.bundle.notes, "words");

        app.execute_vim_delete(
            KeyEvent::from(KeyCode::Char('$')),
            EditorTarget::SessionRename,
        );
        assert_eq!(
            app.session_rename
                .as_ref()
                .map(|rename| rename.name.as_str()),
            Some("rename ")
        );
    }

    #[tokio::test]
    async fn multibyte_deletion_keeps_target_cursors_on_char_boundaries() {
        let mut app = test_app();
        app.composer = "aé🙂".to_string();
        app.composer_cursor = 1;
        app.bundle.notes = "é🙂z".to_string();
        app.notes_cursor = 0;
        app.session_rename = Some(SessionRenameState {
            session_id: Uuid::new_v4(),
            name: "é🙂z".to_string(),
            cursor: 0,
        });

        let _ = app
            .handle_vim_normal_key(KeyEvent::from(KeyCode::Char('x')), EditorTarget::Composer)
            .await;
        assert_eq!(app.composer, "a🙂");
        assert!(app.composer.is_char_boundary(app.composer_cursor));

        let _ = app
            .handle_vim_normal_key(KeyEvent::from(KeyCode::Char('x')), EditorTarget::Notes)
            .await;
        assert_eq!(app.bundle.notes, "🙂z");
        assert!(app.bundle.notes.is_char_boundary(app.notes_cursor));

        let _ = app
            .handle_vim_normal_key(
                KeyEvent::from(KeyCode::Char('x')),
                EditorTarget::SessionRename,
            )
            .await;
        let rename = app.session_rename.as_ref().unwrap();
        assert_eq!(rename.name, "🙂z");
        assert!(rename.name.is_char_boundary(rename.cursor));
    }

    #[test]
    fn notes_pane_uses_notes_editor_title() {
        let mut app = test_app();
        app.selected_pane = Pane::Notes;
        app.editor_mode = ComposerEditorMode::Vim(VimMode::Normal);

        assert_eq!(app.editor_panel_title(), "Notes Editor [vim normal]");
    }

    #[test]
    fn notes_main_focus_activation_respects_editor_mode() {
        let mut app = test_app();
        app.selected_pane = Pane::Notes;

        app.editor_mode = ComposerEditorMode::Standard;
        assert!(app.should_activate_notes_editor_from_main(KeyEvent::from(KeyCode::Char('a'))));
        assert!(!app.should_activate_notes_editor_from_main(KeyEvent::from(KeyCode::Char('1'))));

        app.editor_mode = ComposerEditorMode::Vim(VimMode::Normal);
        assert!(app.should_activate_notes_editor_from_main(KeyEvent::from(KeyCode::Char('h'))));
        assert!(!app.should_activate_notes_editor_from_main(KeyEvent::from(KeyCode::Char('1'))));
    }
}
