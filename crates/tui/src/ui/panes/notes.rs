use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Paragraph, Wrap},
};

use crate::{app::App, editor::render_editor_buffer, ui::panel_block};

impl App {
    pub(crate) fn render_notes(&self, frame: &mut Frame, area: Rect) {
        let active = matches!(
            self.focus,
            crate::model::Focus::Main | crate::model::Focus::Composer
        );
        frame.render_widget(
            Paragraph::new(render_editor_buffer(
                &self.bundle.notes,
                self.notes_cursor.min(self.bundle.notes.len()),
                active,
                self.editor_mode,
            ))
            .block(panel_block(&self.editor_panel_title(), active))
            .wrap(Wrap { trim: false }),
            area,
        );
    }
}
