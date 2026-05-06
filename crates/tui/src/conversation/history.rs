use std::collections::HashMap;

use db::models::execution_process::{ExecutionProcess, ExecutionProcessStatus};
use uuid::Uuid;

pub fn initial_conversation_process_ids(
    process_order: &[Uuid],
    process_map: &HashMap<Uuid, ExecutionProcess>,
) -> Vec<Uuid> {
    const MIN_INITIAL_ENTRIES: usize = 10;

    let mut selected = Vec::new();
    let mut estimated_entries = 0usize;
    for process_id in process_order.iter().rev() {
        let Some(process) = process_map.get(process_id) else {
            continue;
        };
        selected.push(*process_id);
        estimated_entries += 4;
        if process.status == ExecutionProcessStatus::Running && selected.len() == 1 {
            continue;
        }
        if estimated_entries >= MIN_INITIAL_ENTRIES {
            break;
        }
    }
    selected.reverse();
    selected
}

pub fn process_prompt(process: &ExecutionProcess) -> Option<String> {
    use executors::actions::ExecutorActionType;

    let db::models::execution_process::ExecutorActionField::ExecutorAction(action) =
        &process.executor_action.0
    else {
        return None;
    };

    let prompt = match action.typ() {
        ExecutorActionType::CodingAgentInitialRequest(request) => Some(request.prompt.as_str()),
        ExecutorActionType::CodingAgentFollowUpRequest(request) => Some(request.prompt.as_str()),
        ExecutorActionType::ReviewRequest(request) => Some(request.prompt.as_str()),
        ExecutorActionType::ScriptRequest(_) => None,
    }?;

    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
