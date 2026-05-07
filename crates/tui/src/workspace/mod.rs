use db::models::{session::Session, workspace::WorkspaceWithStatus};
use uuid::Uuid;

pub(crate) enum WorkspaceRow<'a> {
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
pub mod list;
pub mod session;
