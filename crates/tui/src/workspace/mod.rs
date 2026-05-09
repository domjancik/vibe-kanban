use db::models::{session::Session, workspace::WorkspaceWithStatus};
use uuid::Uuid;

pub(crate) enum WorkspaceRow<'a> {
    NewWorkspace,
    Header(&'static str),
    Workspace(&'a WorkspaceWithStatus),
}

pub(crate) enum SessionRow<'a> {
    NewSession,
    Session(&'a Session),
}

#[derive(Clone, Copy)]
pub(crate) enum SessionTarget {
    NewSession,
    Existing(Uuid),
}

pub(crate) fn session_target(row: &SessionRow<'_>) -> Option<SessionTarget> {
    match row {
        SessionRow::NewSession => Some(SessionTarget::NewSession),
        SessionRow::Session(session) => Some(SessionTarget::Existing(session.id)),
    }
}

pub mod actions;
pub mod create;
pub mod list;
pub mod session;

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use db::models::session::Session;
    use uuid::Uuid;

    use super::{SessionRow, SessionTarget, session_target};

    fn session(id: Uuid) -> Session {
        let now = Utc.timestamp_opt(1, 0).unwrap();
        Session {
            id,
            workspace_id: Uuid::new_v4(),
            name: Some("session".to_string()),
            executor: Some("CODEX".to_string()),
            agent_working_dir: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn session_target_maps_new_and_existing_rows() {
        assert!(matches!(
            session_target(&SessionRow::NewSession),
            Some(SessionTarget::NewSession)
        ));

        let existing = session(Uuid::new_v4());
        assert!(matches!(
            session_target(&SessionRow::Session(&existing)),
            Some(SessionTarget::Existing(id)) if id == existing.id
        ));
    }
}
