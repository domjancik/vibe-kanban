use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Paragraph, Wrap},
};

use crate::{app::App, editor::render_editor_buffer, ui::panel_block};

impl App {
    pub(crate) fn render_notes(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(render_editor_buffer(
                &self.bundle.notes,
                self.notes_cursor.min(self.bundle.notes.len()),
                self.focus == crate::model::Focus::Composer,
                self.editor_mode,
            ))
                .block(panel_block(
                    &self.editor_panel_title(),
                    self.focus == crate::model::Focus::Composer,
                ))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}
