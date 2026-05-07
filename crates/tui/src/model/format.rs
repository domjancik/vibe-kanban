use chrono::{DateTime, Utc};
use db::models::workspace::Workspace;
use executors::{
    logs::{ActionType, NormalizedEntry, NormalizedEntryError, NormalizedEntryType, ToolStatus},
    model_selector::PermissionPolicy,
};

use crate::model::{DiffChangeKind, LocalDiff, PatchType};

pub fn diff_title(diff: &LocalDiff) -> String {
    diff.new_path
        .clone()
        .or_else(|| diff.old_path.clone())
        .unwrap_or_else(|| "<unknown>".to_string())
}

pub fn format_patch_entry(entry: &PatchType) -> String {
    match entry {
        PatchType::Stdout(s) => s.clone(),
        PatchType::Stderr(s) => s.clone(),
        PatchType::Diff(diff) => format!(
            "{} {}",
            match diff.change {
                DiffChangeKind::Added => "added",
                DiffChangeKind::Deleted => "deleted",
                DiffChangeKind::Modified => "modified",
                DiffChangeKind::Renamed => "renamed",
                DiffChangeKind::Copied => "copied",
                DiffChangeKind::PermissionChange => "chmod",
            },
            diff_title(diff)
        ),
        PatchType::NormalizedEntry(entry) => format_normalized_entry(entry),
    }
}

pub fn format_normalized_entry(entry: &NormalizedEntry) -> String {
    let prefix = match &entry.entry_type {
        NormalizedEntryType::UserMessage => "user",
        NormalizedEntryType::AssistantMessage => "assistant",
        NormalizedEntryType::SystemMessage => "system",
        NormalizedEntryType::Thinking => "thinking",
        NormalizedEntryType::Loading => "loading",
        NormalizedEntryType::UserFeedback { .. } => "feedback",
        NormalizedEntryType::ErrorMessage { error_type } => match error_type {
            NormalizedEntryError::SetupRequired => "setup",
            NormalizedEntryError::Other => "error",
        },
        NormalizedEntryType::NextAction { failed, .. } => {
            if *failed {
                "next action failed"
            } else {
                "next action"
            }
        }
        NormalizedEntryType::ToolUse {
            tool_name,
            action_type,
            status,
        } => return format_tool_use(tool_name, action_type, status, &entry.content),
        NormalizedEntryType::TokenUsageInfo(info) => {
            return format!(
                "context: {} / {}",
                info.total_tokens, info.model_context_window
            );
        }
        NormalizedEntryType::UserAnsweredQuestions { answers } => {
            return answers
                .iter()
                .map(|item| format!("{}: {}", item.question, item.answer.join(", ")))
                .collect::<Vec<_>>()
                .join("\n");
        }
    };

    if entry.content.trim().is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {}", entry.content.trim())
    }
}

fn format_tool_use(
    tool_name: &str,
    action_type: &ActionType,
    status: &ToolStatus,
    content: &str,
) -> String {
    let action = match action_type {
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
    };
    let status = match status {
        ToolStatus::Created => "created",
        ToolStatus::Success => "success",
        ToolStatus::Failed => "failed",
        ToolStatus::Denied { .. } => "denied",
        ToolStatus::PendingApproval { .. } => "approval",
        ToolStatus::TimedOut => "timed out",
    };
    if content.trim().is_empty() {
        format!("tool {tool_name} [{status}] {action}")
    } else {
        format!("tool {tool_name} [{status}] {action}\n{}", content.trim())
    }
}

pub fn format_relative_time(ts: Option<DateTime<Utc>>) -> String {
    let Some(ts) = ts else {
        return "never".to_string();
    };
    let delta = Utc::now().signed_duration_since(ts);
    if delta.num_seconds() < 60 {
        format!("{}s ago", delta.num_seconds().max(0))
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

pub fn workspace_title(workspace: &Workspace) -> String {
    workspace
        .name
        .clone()
        .unwrap_or_else(|| workspace.branch.clone())
}

pub fn display_variant(variant: Option<&str>) -> &str {
    variant.unwrap_or("DEFAULT")
}

pub fn display_permission(policy: Option<&PermissionPolicy>) -> &str {
    match policy {
        Some(PermissionPolicy::Auto) => "AUTO",
        Some(PermissionPolicy::Supervised) => "SUPERVISED",
        Some(PermissionPolicy::Plan) => "PLAN",
        None => "DEFAULT",
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use executors::logs::{
        ActionType, AnsweredQuestion, NormalizedEntry, NormalizedEntryError, NormalizedEntryType,
        TokenUsageInfo, ToolStatus,
    };
    use uuid::Uuid;

    use super::{
        diff_title, display_permission, display_variant, format_normalized_entry,
        format_patch_entry, format_relative_time, workspace_title,
    };
    use crate::model::{DiffChangeKind, LocalDiff, PatchType};

    #[test]
    fn diff_and_patch_formatters_cover_primary_variants() {
        let diff = LocalDiff {
            change: DiffChangeKind::Renamed,
            old_path: Some("old.rs".to_string()),
            new_path: Some("new.rs".to_string()),
            old_content: None,
            new_content: None,
            content_omitted: false,
            additions: None,
            deletions: None,
            repo_id: Some(Uuid::new_v4()),
        };
        assert_eq!(diff_title(&diff), "new.rs");
        assert_eq!(format_patch_entry(&PatchType::Diff(diff)), "renamed new.rs");
        assert_eq!(
            format_patch_entry(&PatchType::Stdout("out".to_string())),
            "out".to_string()
        );
    }

    #[test]
    fn normalized_entry_formatting_handles_edge_cases() {
        let empty = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::AssistantMessage,
            content: "   ".to_string(),
            metadata: None,
        };
        assert_eq!(format_normalized_entry(&empty), "assistant");

        let tokens = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::TokenUsageInfo(TokenUsageInfo {
                total_tokens: 12,
                model_context_window: 100,
            }),
            content: String::new(),
            metadata: None,
        };
        assert_eq!(format_normalized_entry(&tokens), "context: 12 / 100");

        let error = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::ErrorMessage {
                error_type: NormalizedEntryError::SetupRequired,
            },
            content: "needs setup".to_string(),
            metadata: None,
        };
        assert_eq!(format_normalized_entry(&error), "setup: needs setup");

        let answers = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::UserAnsweredQuestions {
                answers: vec![AnsweredQuestion {
                    question: "Deploy?".to_string(),
                    answer: vec!["yes".to_string(), "prod".to_string()],
                }],
            },
            content: String::new(),
            metadata: None,
        };
        assert_eq!(format_normalized_entry(&answers), "Deploy?: yes, prod");

        let tool = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::ToolUse {
                tool_name: "shell".to_string(),
                action_type: ActionType::CommandRun {
                    command: "cargo test".to_string(),
                    result: None,
                    category: Default::default(),
                },
                status: ToolStatus::Success,
            },
            content: "done".to_string(),
            metadata: None,
        };
        assert!(format_normalized_entry(&tool).contains("tool shell [success] cmd `cargo test`"));
    }

    #[test]
    fn relative_and_display_helpers_return_expected_labels() {
        assert_eq!(format_relative_time(None), "never");
        assert_eq!(
            format_relative_time(Some(Utc::now() - Duration::seconds(5))),
            "5s ago"
        );
        assert_eq!(
            format_relative_time(Some(Utc::now() - Duration::minutes(2))),
            "2m ago"
        );
        assert_eq!(
            format_relative_time(Some(Utc::now() - Duration::hours(3))),
            "3h ago"
        );
        assert_eq!(
            format_relative_time(Some(Utc::now() - Duration::days(4))),
            "4d ago"
        );

        let workspace = db::models::workspace::Workspace {
            id: Uuid::new_v4(),
            task_id: None,
            container_ref: None,
            branch: "feature/test".to_string(),
            setup_completed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: false,
            pinned: false,
            name: Some("Named".to_string()),
            worktree_deleted: false,
        };
        assert_eq!(workspace_title(&workspace), "Named");
        assert_eq!(display_variant(None), "DEFAULT");
        assert_eq!(display_variant(Some("PLAN")), "PLAN");
        assert_eq!(display_permission(None), "DEFAULT");
        assert_eq!(
            display_permission(Some(&executors::model_selector::PermissionPolicy::Plan)),
            "PLAN"
        );
    }
}
