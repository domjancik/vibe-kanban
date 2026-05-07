use ratatui::{
    Frame,
    layout::Rect,
    text::Text,
    widgets::{Paragraph, Wrap},
};

use crate::{
    app::App,
    conversation::render_log_entry,
    model::{Focus, LogsPaneRenderCache},
    ui::{panel_block, render_vertical_scrollbar},
};

impl App {
    fn logs_pane_cache(&mut self) -> &LogsPaneRenderCache {
        if self
            .bundle
            .logs_cache
            .as_ref()
            .is_none_or(|cache| cache.revision != self.bundle.logs_revision)
        {
            let lines = self
                .bundle
                .log_entries
                .iter()
                .enumerate()
                .flat_map(|(index, entry)| render_log_entry(index, entry))
                .collect::<Vec<_>>();
            self.bundle.logs_cache = Some(LogsPaneRenderCache {
                revision: self.bundle.logs_revision,
                total_lines: lines.len(),
                lines,
            });
        }
        self.bundle
            .logs_cache
            .as_ref()
            .expect("logs pane cache should be populated")
    }

    pub(crate) fn render_logs(&mut self, frame: &mut Frame, area: Rect) {
        let cache = self.logs_pane_cache().clone();
        frame.render_widget(
            Paragraph::new(Text::from(cache.lines.clone()))
                .block(panel_block("Logs", self.focus == Focus::Main))
                .scroll((self.bundle.log_scroll, 0))
                .wrap(Wrap { trim: false }),
            area,
        );
        render_vertical_scrollbar(
            frame,
            area,
            cache.total_lines,
            area.height.saturating_sub(2) as usize,
            self.bundle.log_scroll as usize,
        );
    }
}
