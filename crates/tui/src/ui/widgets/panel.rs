use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders},
};

pub fn panel_block<'a>(title: &'a str, active: bool) -> Block<'a> {
    let style = if active {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Span::styled(title.to_string(), style))
}

#[cfg(test)]
mod tests {
    use ratatui::{
        Terminal,
        backend::TestBackend,
        style::Color,
        widgets::{Paragraph, Widget},
    };

    use super::panel_block;

    #[test]
    fn panel_block_renders_distinct_active_and_inactive_borders() {
        let backend = TestBackend::new(12, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                Paragraph::new("")
                    .block(panel_block("Active", true))
                    .render(frame.area(), frame.buffer_mut());
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer().cell((0, 0)).unwrap().fg,
            Color::Cyan
        );

        terminal
            .draw(|frame| {
                Paragraph::new("")
                    .block(panel_block("Idle", false))
                    .render(frame.area(), frame.buffer_mut());
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer().cell((0, 0)).unwrap().fg,
            Color::DarkGray
        );
    }
}
