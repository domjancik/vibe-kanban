use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Color, Style},
    text::Line,
    widgets::Paragraph,
};

pub fn render_vertical_scrollbar(
    frame: &mut Frame,
    area: Rect,
    total_items: usize,
    viewport_items: usize,
    offset: usize,
) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 0,
    });
    if inner.height == 0 || inner.width == 0 || total_items <= viewport_items || viewport_items == 0
    {
        return;
    }

    let track_height = inner.height as usize;
    let thumb_height =
        ((viewport_items * track_height).div_ceil(total_items)).clamp(1, track_height);
    let max_offset = total_items.saturating_sub(viewport_items).max(1);
    let max_thumb_top = track_height.saturating_sub(thumb_height);
    let thumb_top = (offset.min(max_offset) * max_thumb_top) / max_offset;

    let lines = (0..track_height)
        .map(|index| {
            if index >= thumb_top && index < thumb_top + thumb_height {
                Line::styled("┃", Style::default().fg(Color::Cyan))
            } else {
                Line::styled("│", Style::default().fg(Color::DarkGray))
            }
        })
        .collect::<Vec<_>>();

    let scrollbar_area = Rect {
        x: inner.right().saturating_sub(1),
        y: inner.y,
        width: 1,
        height: inner.height,
    };
    frame.render_widget(Paragraph::new(lines), scrollbar_area);
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};

    use super::render_vertical_scrollbar;

    #[test]
    fn scrollbar_renders_thumb_and_track_when_needed() {
        let backend = TestBackend::new(4, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_vertical_scrollbar(frame, Rect::new(0, 0, 4, 8), 20, 5, 5);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let glyphs = (1..7)
            .map(|y| buffer.cell((3, y)).unwrap().symbol())
            .collect::<Vec<_>>();
        assert!(glyphs.iter().any(|symbol| *symbol == "┃"));
        assert!(glyphs.iter().any(|symbol| *symbol == "│"));
    }
}
