use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::App,
    editor::render_editor_buffer,
    ui::{centered_rect, panel_block},
};

impl App {
    pub(crate) fn render_search_prompt(&self, frame: &mut Frame, area: Rect) {
        let Some(prompt) = self.search_prompt.as_ref() else {
            return;
        };
        let popup = centered_rect(60, 18, area);
        let label = match prompt.target {
            crate::app::SearchTarget::Workspaces => "Workspace Filter",
            crate::app::SearchTarget::Sessions => "Session Filter",
            crate::app::SearchTarget::Conversation => "Conversation Search",
        };

        frame.render_widget(Clear, popup);
        frame.render_widget(panel_block(label, true), popup);
        frame.render_widget(
            Paragraph::new(render_editor_buffer(
                &prompt.query,
                prompt.cursor,
                true,
                self.editor_mode,
            ))
            .block(panel_block("/", false))
            .wrap(Wrap { trim: false }),
            popup.inner(Margin {
                vertical: 1,
                horizontal: 2,
            }),
        );
    }

    pub(crate) fn render_agent_picker(&self, frame: &mut Frame, area: Rect) {
        let Some(picker) = self.agent_picker.as_ref() else {
            return;
        };
        let popup = centered_rect(72, 55, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(6)])
            .split(popup);
        let options = self.filtered_agent_mode_options();
        let selected = picker.selected.min(options.len().saturating_sub(1));
        let items = options
            .iter()
            .map(|option| match option {
                None => ListItem::new(vec![
                    Line::styled(
                        "Default",
                        Style::default()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        "Use the executor default agent mode",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Some(agent) => {
                    let mut lines = vec![Line::from(vec![
                        Span::styled(
                            agent.label.clone(),
                            Style::default()
                                .fg(Color::LightBlue)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(agent.id.clone(), Style::default().fg(Color::Gray)),
                        if agent.is_default {
                            Span::styled("  default", Style::default().fg(Color::Yellow))
                        } else {
                            Span::raw("")
                        },
                    ])];
                    if let Some(description) = &agent.description {
                        lines.push(Line::styled(
                            description.clone(),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    ListItem::new(lines)
                }
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default().with_selected(Some(selected));

        frame.render_widget(Clear, popup);
        frame.render_widget(panel_block("Agent Mode", true), popup);
        frame.render_widget(
            Paragraph::new(format!("Search: {}", picker.query))
                .block(panel_block("Filter", false))
                .wrap(Wrap { trim: false }),
            chunks[0],
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(panel_block("Options", false))
                .highlight_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> "),
            chunks[1],
            &mut state,
        );
    }
}
