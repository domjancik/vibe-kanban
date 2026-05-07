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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::execution_process::{
        ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus, ExecutorActionField,
    };
    use executors::{
        actions::{
            ExecutorAction, ExecutorActionType,
            coding_agent_follow_up::CodingAgentFollowUpRequest,
            coding_agent_initial::CodingAgentInitialRequest,
            review::ReviewRequest,
            script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
        },
        profile::ExecutorConfig,
    };
    use sqlx::types::Json;
    use uuid::Uuid;

    use super::{initial_conversation_process_ids, process_prompt};

    fn process_with_action(
        id: Uuid,
        created_at_second: i64,
        status: ExecutionProcessStatus,
        action: ExecutorAction,
    ) -> ExecutionProcess {
        let session_id = Uuid::new_v4();
        let created_at = Utc.timestamp_opt(created_at_second, 0).unwrap();
        ExecutionProcess {
            id,
            session_id,
            run_reason: ExecutionProcessRunReason::CodingAgent,
            executor_action: Json(ExecutorActionField::ExecutorAction(action)),
            status,
            exit_code: None,
            dropped: false,
            started_at: created_at,
            completed_at: None,
            created_at,
            updated_at: created_at,
        }
    }

    fn initial_request(prompt: &str) -> ExecutorAction {
        ExecutorAction::new(
            ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                prompt: prompt.to_string(),
                executor_config: ExecutorConfig::new(executors::executors::BaseCodingAgent::Codex),
                working_dir: None,
            }),
            None,
        )
    }

    #[test]
    fn initial_conversation_process_ids_selects_recent_processes_until_bootstrap_budget() {
        let ids = [
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        ];
        let mut process_map = HashMap::new();
        for (index, id) in ids.iter().enumerate() {
            process_map.insert(
                *id,
                process_with_action(
                    *id,
                    index as i64,
                    ExecutionProcessStatus::Completed,
                    initial_request(&format!("prompt {index}")),
                ),
            );
        }

        let selected = initial_conversation_process_ids(&ids, &process_map);

        assert_eq!(selected, ids[1..].to_vec());
    }

    #[test]
    fn initial_conversation_process_ids_keeps_newest_running_process() {
        let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let mut process_map = HashMap::new();
        process_map.insert(
            ids[0],
            process_with_action(
                ids[0],
                1,
                ExecutionProcessStatus::Completed,
                initial_request("one"),
            ),
        );
        process_map.insert(
            ids[1],
            process_with_action(
                ids[1],
                2,
                ExecutionProcessStatus::Completed,
                initial_request("two"),
            ),
        );
        process_map.insert(
            ids[2],
            process_with_action(
                ids[2],
                3,
                ExecutionProcessStatus::Running,
                initial_request("run"),
            ),
        );

        let selected = initial_conversation_process_ids(&ids, &process_map);

        assert_eq!(selected, ids.to_vec());
        assert_eq!(selected.last(), Some(&ids[2]));
    }

    #[test]
    fn initial_conversation_process_ids_skips_missing_processes() {
        let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let mut process_map = HashMap::new();
        process_map.insert(
            ids[0],
            process_with_action(
                ids[0],
                1,
                ExecutionProcessStatus::Completed,
                initial_request("one"),
            ),
        );
        process_map.insert(
            ids[2],
            process_with_action(
                ids[2],
                3,
                ExecutionProcessStatus::Completed,
                initial_request("three"),
            ),
        );

        let selected = initial_conversation_process_ids(&ids, &process_map);

        assert_eq!(selected, vec![ids[0], ids[2]]);
    }

    #[test]
    fn process_prompt_extracts_supported_action_prompts() {
        let process = process_with_action(
            Uuid::new_v4(),
            1,
            ExecutionProcessStatus::Completed,
            ExecutorAction::new(
                ExecutorActionType::CodingAgentFollowUpRequest(CodingAgentFollowUpRequest {
                    prompt: " follow up ".to_string(),
                    session_id: "agent-session".to_string(),
                    reset_to_message_id: None,
                    executor_config: ExecutorConfig::new(
                        executors::executors::BaseCodingAgent::Codex,
                    ),
                    working_dir: None,
                }),
                None,
            ),
        );
        assert_eq!(process_prompt(&process).as_deref(), Some("follow up"));

        let review = process_with_action(
            Uuid::new_v4(),
            2,
            ExecutionProcessStatus::Completed,
            ExecutorAction::new(
                ExecutorActionType::ReviewRequest(ReviewRequest {
                    executor_config: ExecutorConfig::new(
                        executors::executors::BaseCodingAgent::Codex,
                    ),
                    context: None,
                    prompt: "review me".to_string(),
                    session_id: None,
                    working_dir: None,
                }),
                None,
            ),
        );
        assert_eq!(process_prompt(&review).as_deref(), Some("review me"));
    }

    #[test]
    fn process_prompt_returns_none_for_script_actions_and_blank_prompts() {
        let script = process_with_action(
            Uuid::new_v4(),
            1,
            ExecutionProcessStatus::Completed,
            ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: "echo hi".to_string(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::SetupScript,
                    working_dir: None,
                }),
                None,
            ),
        );
        assert_eq!(process_prompt(&script), None);

        let blank = process_with_action(
            Uuid::new_v4(),
            2,
            ExecutionProcessStatus::Completed,
            initial_request("   "),
        );
        assert_eq!(process_prompt(&blank), None);
    }
}
