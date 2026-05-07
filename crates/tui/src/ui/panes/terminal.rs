use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    app::App,
    model::Focus,
    ui::{centered_rect, panel_block, terminal_content_area},
};

impl App {
    pub(crate) fn render_terminal(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.bundle.terminal.input_mode {
            "Terminal *"
        } else {
            "Terminal"
        };
        let block = panel_block(title, self.focus == Focus::Main);
        let content_area = terminal_content_area(area);
        self.handle_terminal_resize(content_area);
        let screen = self.bundle.terminal.parser.screen();
        let mut lines = Vec::new();
        for row in 0..screen.size().0 {
            let mut spans = Vec::new();
            for col in 0..screen.size().1 {
                if let Some(cell) = screen.cell(row, col) {
                    spans.push(Span::styled(
                        cell.contents().chars().next().unwrap_or(' ').to_string(),
                        terminal_cell_style(cell),
                    ));
                }
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(Text::from(lines)), content_area);
        if let Some(error) = &self.bundle.terminal.error {
            let popup = centered_rect(70, 20, area);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(error.as_str())
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("Terminal error"),
                    )
                    .wrap(Wrap { trim: false }),
                popup,
            );
        }
    }
}

fn terminal_cell_style(cell: &vt100::Cell) -> Style {
    let mut fg = vt100_color_to_ratatui(cell.fgcolor());
    let mut bg = vt100_color_to_ratatui(cell.bgcolor());
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }

    let mut style = Style::default().fg(fg).bg(bg);
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(idx) => match idx {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            other => Color::Indexed(other),
        },
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
