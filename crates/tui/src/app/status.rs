use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    app::App,
    model::{QueueStatus, display_permission, display_variant},
};

impl App {
    pub(crate) fn composer_selection_line(&self) -> Line<'static> {
        let config = self.composer_config.as_ref();
        let executor = config
            .map(|config| config.executor.to_string())
            .unwrap_or_else(|| "loading".to_string());
        let executor_locked = !self.creating_new_session && self.current_session().is_some();
        let variant = config
            .map(|config| display_variant(config.variant.as_deref()).to_string())
            .unwrap_or_else(|| "loading".to_string());
        let model = self
            .selected_model_label()
            .unwrap_or_else(|| "default".to_string());
        let reasoning = self
            .selected_reasoning_label()
            .unwrap_or_else(|| "default".to_string());
        let agent_mode = self.selected_agent_mode_label();
        let permission = config
            .map(|config| display_permission(config.permission_policy.as_ref()).to_string())
            .unwrap_or_else(|| "default".to_string());
        let shortcut_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED);
        let label_style = Style::default().fg(Color::DarkGray);

        Line::from(vec![
            Span::styled("E", shortcut_style),
            Span::styled("xec ", label_style),
            Span::styled(executor, Style::default().fg(Color::Cyan)),
            if executor_locked {
                Span::styled(" (locked)", Style::default().fg(Color::DarkGray))
            } else {
                Span::raw("")
            },
            Span::raw("  "),
            Span::styled("V", shortcut_style),
            Span::styled("ar ", label_style),
            Span::styled(variant, Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled("M", shortcut_style),
            Span::styled("odel ", label_style),
            Span::styled(model, Style::default().fg(Color::Green)),
            Span::raw("  "),
            Span::styled("R", shortcut_style),
            Span::styled("sn ", label_style),
            Span::styled(reasoning, Style::default().fg(Color::Magenta)),
            Span::raw("  "),
            Span::styled("A", shortcut_style),
            Span::styled("gt ", label_style),
            Span::styled(agent_mode, Style::default().fg(Color::LightBlue)),
            Span::raw("  "),
            Span::styled("P", shortcut_style),
            Span::styled("erm ", label_style),
            Span::styled(permission, Style::default().fg(Color::LightRed)),
        ])
    }

    pub(crate) fn composer_status_line(&self) -> Line<'static> {
        let draft = self.draft_status_label();
        let queue = self.queue_status_label();
        let draft_style = if self.composer_queue_conflict {
            Style::default().fg(Color::Yellow)
        } else if self.composer_dirty {
            Style::default().fg(Color::LightBlue)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let queue_style = if self.queue_pending {
            Style::default().fg(Color::Yellow)
        } else if self.is_queue_present() {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut spans = vec![
            Span::styled("Draft ", Style::default().fg(Color::Gray)),
            Span::styled(draft, draft_style),
            Span::raw("  "),
            Span::styled("Queue ", Style::default().fg(Color::Gray)),
            Span::styled(queue, queue_style),
            Span::raw("  "),
            Span::styled(
                "F2",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" editor mode ", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(
                "Q",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" queue ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "X",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "D",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" discard", Style::default().fg(Color::DarkGray)),
        ];
        if self.creating_new_session {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                " create session ",
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                " cancel",
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }

    pub(crate) fn draft_status_label(&self) -> String {
        if self.composer_queue_conflict {
            "blocked by queued follow-up".to_string()
        } else if self.composer_dirty {
            "saving...".to_string()
        } else if self.composer_scratch_loaded {
            "synced".to_string()
        } else {
            "loading".to_string()
        }
    }

    pub(crate) fn queue_status_label(&self) -> String {
        if self.queue_pending {
            return "loading".to_string();
        }
        match &self.queue_status {
            QueueStatus::Empty => "not queued".to_string(),
            QueueStatus::Queued { message } => {
                format!("queued at {}", message.queued_at.format("%H:%M:%S"))
            }
        }
    }
}
