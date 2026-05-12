use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState},
};

use crate::{
    app::App,
    model::{Focus, GitPaneRenderCache, Merge, MergeStatus},
    ui::{panel_block, render_vertical_scrollbar},
};

impl App {
    fn git_pane_cache(&mut self) -> &GitPaneRenderCache {
        let selected_repo_index = self
            .bundle
            .selected_git_repo_index
            .min(self.bundle.git_status.len().saturating_sub(1));
        let needs_rebuild = self.bundle.git_cache.as_ref().is_none_or(|cache| {
            cache.revision != self.bundle.git_revision
                || cache.selected_repo_index != selected_repo_index
        });
        if needs_rebuild {
            let items = if self.bundle.git_status.is_empty() {
                vec![ListItem::new(Line::raw("No repository status available"))]
            } else {
                self.bundle
                    .git_status
                    .iter()
                    .map(|status| {
                        let (pr_label, pr_style) = status
                            .status
                            .merges
                            .iter()
                            .find_map(|merge| match merge {
                                Merge::Pr(pr) => Some(match pr.pr_info.status {
                                    MergeStatus::Open => (
                                        format!("PR #{} open", pr.pr_info.pr_number),
                                        Style::default().fg(Color::Green),
                                    ),
                                    MergeStatus::Merged => (
                                        format!("PR #{} merged", pr.pr_info.pr_number),
                                        Style::default().fg(Color::Blue),
                                    ),
                                    MergeStatus::Closed => (
                                        format!("PR #{} closed", pr.pr_info.pr_number),
                                        Style::default().fg(Color::Yellow),
                                    ),
                                    MergeStatus::Unknown => (
                                        format!("PR #{} linked", pr.pr_info.pr_number),
                                        Style::default().fg(Color::Gray),
                                    ),
                                }),
                                _ => None,
                            })
                            .unwrap_or(("No PR".to_string(), Style::default().fg(Color::DarkGray)));

                        ListItem::new(vec![
                            Line::styled(
                                status.repo_name.clone(),
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Line::raw(format!(
                                "target {}  ahead {}  behind {}",
                                status.status.target_branch_name,
                                status.status.commits_ahead.unwrap_or_default(),
                                status.status.commits_behind.unwrap_or_default()
                            )),
                            Line::from(vec![
                                Span::styled(pr_label, pr_style),
                                Span::raw("  "),
                                Span::styled(
                                    format!(
                                        "uncommitted {}  untracked {}",
                                        status.status.uncommitted_count.unwrap_or_default(),
                                        status.status.untracked_count.unwrap_or_default()
                                    ),
                                    Style::default().fg(Color::DarkGray),
                                ),
                            ]),
                            Line::styled(
                                "p primary PR  o open  a attach".to_string(),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Line::raw(""),
                        ])
                    })
                    .collect()
            };
            self.bundle.git_cache = Some(GitPaneRenderCache {
                revision: self.bundle.git_revision,
                selected_repo_index,
                items,
            });
        }
        self.bundle
            .git_cache
            .as_ref()
            .expect("git pane cache should be populated")
    }

    pub(crate) fn render_git(&mut self, frame: &mut Frame, area: Rect) {
        let cache = self.git_pane_cache().clone();
        let mut state = ListState::default();
        if !self.bundle.git_status.is_empty() {
            state.select(Some(
                self.bundle
                    .selected_git_repo_index
                    .min(self.bundle.git_status.len().saturating_sub(1)),
            ));
        }
        frame.render_stateful_widget(
            List::new(cache.items)
                .block(panel_block("Git", self.focus == Focus::Main))
                .highlight_style(
                    Style::default()
                        .bg(Color::Rgb(28, 38, 48))
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            area,
            &mut state,
        );
        render_vertical_scrollbar(
            frame,
            area,
            self.bundle.git_status.len().max(1),
            crate::app::viewport_capacity(area, 5),
            crate::app::selected_list_offset(
                self.bundle.selected_git_repo_index,
                self.bundle.git_status.len(),
                crate::app::viewport_capacity(area, 5),
            ),
        );
    }
}
