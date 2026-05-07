use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
};

use crate::{
    app::App,
    model::{Focus, GitPaneRenderCache, Merge},
    ui::{panel_block, render_vertical_scrollbar},
};

impl App {
    fn git_pane_cache(&mut self) -> &GitPaneRenderCache {
        if self
            .bundle
            .git_cache
            .as_ref()
            .is_none_or(|cache| cache.revision != self.bundle.git_revision)
        {
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
                        if let Some(pr) =
                            status.status.merges.iter().find_map(|merge| match merge {
                                Merge::Pr(pr) => Some(pr),
                                _ => None,
                            })
                        {
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
            self.bundle.git_cache = Some(GitPaneRenderCache {
                revision: self.bundle.git_revision,
                total_lines: lines.len(),
                text: Text::from(lines),
            });
        }
        self.bundle
            .git_cache
            .as_ref()
            .expect("git pane cache should be populated")
    }

    pub(crate) fn render_git(&mut self, frame: &mut Frame, area: Rect) {
        let cache = self.git_pane_cache().clone();
        frame.render_widget(
            Paragraph::new(cache.text.clone())
                .block(panel_block("Git", self.focus == Focus::Main))
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
