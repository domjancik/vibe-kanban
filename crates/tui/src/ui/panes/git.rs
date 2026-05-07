use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
};

use crate::{
    app_state::App,
    model::{Focus, Merge},
    ui::{panel_block, render_vertical_scrollbar},
};

impl App {
    pub(crate) fn render_git(&self, frame: &mut Frame, area: Rect) {
        let lines = if self.bundle.git_status.is_empty() {
            vec![Line::raw("No repository status available")]
        } else {
            self.bundle
                .git_status
                .iter()
                .flat_map(|status| {
                    let mut lines = vec![
                        Line::styled(
                            format!(
                                "{} -> {}",
                                status.repo_name, status.status.target_branch_name
                            ),
                            Style::default().fg(Color::Cyan),
                        ),
                        Line::raw(format!(
                            "ahead {}  behind {}  uncommitted {:?}  untracked {:?}",
                            status.status.commits_ahead.unwrap_or_default(),
                            status.status.commits_behind.unwrap_or_default(),
                            status.status.uncommitted_count,
                            status.status.untracked_count
                        )),
                    ];
                    if status.status.is_rebase_in_progress {
                        lines.push(Line::styled(
                            "rebase in progress",
                            Style::default().fg(Color::Yellow),
                        ));
                    }
                    if !status.status.conflicted_files.is_empty() {
                        lines.push(Line::styled(
                            format!("conflicts: {}", status.status.conflicted_files.join(", ")),
                            Style::default().fg(Color::Red),
                        ));
                    }
                    if let Some(pr) = status.status.merges.iter().find_map(|merge| match merge {
                        Merge::Pr(pr) => Some(pr),
                        _ => None,
                    }) {
                        lines.push(Line::raw(format!(
                            "pr #{}  {}",
                            pr.pr_info.pr_number, pr.pr_info.pr_url
                        )));
                    }
                    lines.push(Line::raw(""));
                    lines
                })
                .collect()
        };
        let total_lines = lines.len();
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(panel_block("Git", self.focus == Focus::Main))
                .wrap(Wrap { trim: false }),
            area,
        );
        render_vertical_scrollbar(
            frame,
            area,
            total_lines,
            area.height.saturating_sub(2) as usize,
            self.bundle.log_scroll as usize,
        );
    }
}
