use executors::logs::{ActionType, NormalizedEntry, NormalizedEntryType, ToolStatus};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    conversation::{OptimisticConversationEntry, OptimisticState},
    model::{PatchType, diff_title, format_patch_entry},
};

pub fn render_optimistic_chat_entry(entry: &OptimisticConversationEntry) -> Vec<Line<'static>> {
    let status = match entry.state {
        OptimisticState::Pending => ("sending...", Color::Yellow),
        OptimisticState::Failed => ("failed", Color::Red),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "user",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(status.0, Style::default().fg(status.1)),
        Span::raw(" "),
        Span::styled(
            format!("({})", entry.executor_config.executor),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    lines.extend(
        entry
            .message
            .lines()
            .map(|line| Line::styled(format!("  {line}"), Style::default().fg(Color::White)))
            .collect::<Vec<_>>(),
    );
    lines.push(Line::raw(""));
    lines
}

pub fn render_chat_entry(entry: &PatchType) -> Vec<Line<'static>> {
    match entry {
        PatchType::NormalizedEntry(entry) => render_normalized_chat_entry(entry),
        PatchType::Stdout(output) => indent_chat_lines(vec![
            Line::styled(
                "stdout",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                output.trim_end().to_string(),
                Style::default().fg(Color::Gray),
            ),
            Line::raw(""),
        ]),
        PatchType::Stderr(output) => indent_chat_lines(vec![
            Line::styled(
                "stderr",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                output.trim_end().to_string(),
                Style::default().fg(Color::LightRed),
            ),
            Line::raw(""),
        ]),
        PatchType::Diff(diff) => indent_chat_lines(vec![
            Line::styled(
                format!("diff {}", diff_title(diff)),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(format_patch_entry(entry), Style::default().fg(Color::Gray)),
            Line::raw(""),
        ]),
    }
}

pub fn render_normalized_chat_entry(entry: &NormalizedEntry) -> Vec<Line<'static>> {
    let content = entry.content.trim();
    match &entry.entry_type {
        NormalizedEntryType::ToolUse {
            tool_name,
            action_type,
            status,
        } => {
            let status_color = match status {
                ToolStatus::Success => Color::Green,
                ToolStatus::Failed | ToolStatus::Denied { .. } => Color::Red,
                ToolStatus::PendingApproval { .. } => Color::Yellow,
                ToolStatus::TimedOut => Color::LightRed,
                ToolStatus::Created => Color::Cyan,
            };
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    "tool",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(tool_name.clone(), Style::default().fg(Color::LightCyan)),
                Span::raw(" "),
                Span::styled(
                    format!("[{status:?}]").to_lowercase(),
                    Style::default().fg(status_color),
                ),
                Span::raw(" "),
                Span::styled(
                    summarize_action(action_type),
                    Style::default().fg(Color::Gray),
                ),
            ])];
            if !content.is_empty() {
                lines.extend(
                    content
                        .lines()
                        .map(|line| {
                            Line::styled(format!("  {line}"), Style::default().fg(Color::DarkGray))
                        })
                        .collect::<Vec<_>>(),
                );
            }
            lines.push(Line::raw(""));
            indent_chat_lines(lines)
        }
        NormalizedEntryType::TokenUsageInfo(_) => Vec::new(),
        NormalizedEntryType::UserMessage => render_markdown_labeled_content(
            "user",
            Color::Blue,
            Style::default().fg(Color::Rgb(170, 210, 255)),
            content,
        ),
        NormalizedEntryType::AssistantMessage => render_markdown_labeled_content(
            "assistant",
            Color::Green,
            Style::default().fg(Color::Rgb(180, 255, 190)),
            content,
        ),
        NormalizedEntryType::SystemMessage => {
            render_labeled_content("system", Color::Magenta, content)
        }
        NormalizedEntryType::Thinking => {
            indent_chat_lines(render_labeled_content("thinking", Color::Gray, content))
        }
        NormalizedEntryType::Loading => {
            indent_chat_lines(render_labeled_content("loading", Color::DarkGray, content))
        }
        NormalizedEntryType::UserFeedback { .. } => indent_chat_lines(render_labeled_content(
            "feedback",
            Color::LightBlue,
            content,
        )),
        NormalizedEntryType::ErrorMessage { .. } => {
            indent_chat_lines(render_labeled_content("error", Color::Red, content))
        }
        NormalizedEntryType::NextAction { failed, .. } => {
            let color = if *failed { Color::Red } else { Color::Cyan };
            indent_chat_lines(render_labeled_content("next", color, content))
        }
        NormalizedEntryType::UserAnsweredQuestions { answers } => {
            let text = answers
                .iter()
                .map(|item| format!("{}: {}", item.question, item.answer.join(", ")))
                .collect::<Vec<_>>()
                .join("\n");
            indent_chat_lines(render_labeled_content("answers", Color::LightBlue, &text))
        }
    }
}

pub fn render_log_entry(index: usize, entry: &PatchType) -> Vec<Line<'static>> {
    let style = match entry {
        PatchType::NormalizedEntry(normalized) => match normalized.entry_type {
            NormalizedEntryType::ToolUse { .. } => Style::default().fg(Color::Cyan),
            NormalizedEntryType::TokenUsageInfo(_) => Style::default().fg(Color::Yellow),
            NormalizedEntryType::AssistantMessage => Style::default().fg(Color::Green),
            NormalizedEntryType::UserMessage => Style::default().fg(Color::Blue),
            NormalizedEntryType::ErrorMessage { .. } => Style::default().fg(Color::Red),
            _ => Style::default().fg(Color::Gray),
        },
        PatchType::Stdout(_) => Style::default().fg(Color::Gray),
        PatchType::Stderr(_) => Style::default().fg(Color::Red),
        PatchType::Diff(_) => Style::default().fg(Color::Magenta),
    };

    format_patch_entry(entry)
        .lines()
        .enumerate()
        .map(|(line_index, line)| {
            let prefix = if line_index == 0 {
                format!("{index:04} ")
            } else {
                "     ".to_string()
            };
            Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                Span::styled(line.to_string(), style),
            ])
        })
        .chain(std::iter::once(Line::raw("")))
        .collect()
}

fn indent_chat_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            if line.spans.is_empty() {
                return line;
            }

            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::raw("    "));
            spans.extend(line.spans);
            let mut indented = Line::from(spans);
            indented.style = line.style;
            indented.alignment = line.alignment;
            indented
        })
        .collect()
}

fn render_labeled_content(label: &str, label_color: Color, content: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        label.to_string(),
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD),
    )];
    if content.is_empty() {
        lines.push(Line::styled(
            "  (empty)".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        lines.extend(
            content
                .lines()
                .map(|line| Line::styled(format!("  {line}"), Style::default().fg(Color::White)))
                .collect::<Vec<_>>(),
        );
    }
    lines.push(Line::raw(""));
    lines
}

fn render_markdown_labeled_content(
    label: &str,
    label_color: Color,
    body_style: Style,
    content: &str,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        label.to_string(),
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED),
    )];
    if content.is_empty() {
        lines.push(Line::styled(
            "  (empty)".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        lines.extend(crate::conversation::markdown::render_markdown_lines(
            content, body_style,
        ));
    }
    lines.push(Line::raw(""));
    lines
}

fn summarize_action(action_type: &ActionType) -> String {
    match action_type {
        ActionType::CommandRun { command, .. } => format!("cmd `{command}`"),
        ActionType::FileRead { path } => format!("read {path}"),
        ActionType::FileEdit { path, .. } => format!("edit {path}"),
        ActionType::Search { query } => format!("search {query}"),
        ActionType::WebFetch { url } => format!("fetch {url}"),
        ActionType::Tool { tool_name, .. } => tool_name.clone(),
        ActionType::TaskCreate {
            description,
            subagent_type,
            ..
        } => {
            if let Some(subagent_type) = subagent_type {
                format!("spawn {subagent_type}: {description}")
            } else {
                description.clone()
            }
        }
        ActionType::PlanPresentation { .. } => "present plan".to_string(),
        ActionType::TodoManagement { operation, .. } => format!("todo {operation}"),
        ActionType::AskUserQuestion { .. } => "ask user".to_string(),
        ActionType::Other { description } => description.clone(),
    }
}

#[cfg(test)]
mod tests {
    use executors::logs::{ActionType, NormalizedEntry, NormalizedEntryType, ToolStatus};
    use ratatui::style::{Color, Modifier, Style};

    use crate::conversation::{render_normalized_chat_entry, wrap_line};

    fn entry(entry_type: NormalizedEntryType, content: &str) -> NormalizedEntry {
        NormalizedEntry {
            timestamp: None,
            entry_type,
            content: content.to_string(),
            metadata: None,
        }
    }

    #[test]
    fn user_label_is_rendered_in_blue() {
        let lines = render_normalized_chat_entry(&entry(
            NormalizedEntryType::UserMessage,
            "Hello **world**",
        ));
        let label = &lines[0];
        assert_eq!(label.spans[0].content.as_ref(), "user");
        assert_eq!(label.style.fg, Some(Color::Blue));
        assert!(label.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn assistant_label_is_rendered_in_green() {
        let lines =
            render_normalized_chat_entry(&entry(NormalizedEntryType::AssistantMessage, "Hi there"));
        let label = &lines[0];
        assert_eq!(label.spans[0].content.as_ref(), "assistant");
        assert_eq!(label.style.fg, Some(Color::Green));
        assert!(label.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn wrapped_label_preserves_line_color() {
        let lines = wrap_line(
            ratatui::text::Line::styled(
                "assistant",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            4,
        );
        assert!(lines.iter().all(|line| line.style.fg == Some(Color::Green)));
    }

    #[test]
    fn tool_entries_are_indented_in_chat() {
        let lines = render_normalized_chat_entry(&entry(
            NormalizedEntryType::ToolUse {
                tool_name: "shell".to_string(),
                action_type: ActionType::Other {
                    description: "run".to_string(),
                },
                status: ToolStatus::Success,
            },
            "done",
        ));
        assert_eq!(lines[0].spans[0].content.as_ref(), "    ");
        assert_eq!(lines[0].spans[1].content.as_ref(), "tool");
    }
}
