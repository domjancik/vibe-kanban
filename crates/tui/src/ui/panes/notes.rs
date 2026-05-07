use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Paragraph, Wrap},
};

use crate::{app_state::App, model::Focus, ui::panel_block};

impl App {
    pub(crate) fn render_notes(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(self.bundle.notes.as_str())
                .block(panel_block("Notes", self.focus == Focus::Main))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}
