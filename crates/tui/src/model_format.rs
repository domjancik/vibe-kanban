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
