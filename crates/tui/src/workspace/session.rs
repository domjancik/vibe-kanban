use db::models::session::Session;
use ratatui::layout::Rect;

use crate::{
    app::App,
    workspace::{SessionRow, session_target},
};

impl App {
    pub(crate) fn switch_session_or_process(&mut self, size: Rect) {
        let _ = size;
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows).unwrap_or(0);
        if let Some(row) = rows.get(index)
            && let Some(target) = session_target(row)
        {
            self.select_session_target(target);
        }
    }

    pub(crate) fn session_rows(&self) -> Vec<SessionRow<'_>> {
        let mut rows = Vec::with_capacity(self.bundle.sessions.len() + 1);
        rows.push(SessionRow::NewSession);
        rows.extend(self.bundle.sessions.iter().map(SessionRow::Session));
        rows
    }

    pub(crate) fn selected_session_row_index(&self, rows: &[SessionRow<'_>]) -> Option<usize> {
        if self.creating_new_session {
            return Some(0);
        }
        let selected = self.bundle.selected_session_id?;
        rows.iter().position(|row| match row {
            SessionRow::NewSession => false,
            SessionRow::Session(session) => session.id == selected,
        })
    }

    pub(crate) fn selected_session_for_rename(&self) -> Option<&Session> {
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows)?;
        match rows.get(index)? {
            SessionRow::Session(session) => Some(session),
            SessionRow::NewSession => None,
        }
    }

    pub(crate) fn current_session(&self) -> Option<&Session> {
        self.bundle
            .selected_session_id
            .and_then(|id| self.bundle.sessions.iter().find(|session| session.id == id))
    }
}
