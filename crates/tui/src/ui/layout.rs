use ratatui::{
    layout::{Constraint, Direction, Layout, Rect, Size},
    prelude::Margin,
};

pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}

pub fn rect_from_size(size: Size) -> Rect {
    Rect::new(0, 0, size.width, size.height)
}

pub fn terminal_content_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
    .inner(Margin {
        vertical: 0,
        horizontal: 1,
    })
}

#[cfg(test)]
mod tests {
    use ratatui::{layout::Rect, prelude::Size};

    use super::{centered_rect, rect_from_size, terminal_content_area};

    #[test]
    fn centered_rect_returns_expected_inner_area() {
        assert_eq!(
            centered_rect(50, 40, Rect::new(0, 0, 100, 50)),
            Rect::new(25, 15, 50, 20)
        );
    }

    #[test]
    fn size_and_terminal_helpers_preserve_expected_insets() {
        assert_eq!(rect_from_size(Size::new(80, 24)), Rect::new(0, 0, 80, 24));
        assert_eq!(
            terminal_content_area(Rect::new(0, 0, 20, 10)),
            Rect::new(2, 1, 16, 8)
        );
    }
}
