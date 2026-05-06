use crate::model::Focus;

pub fn next_focus(current: &Focus) -> Focus {
    match current {
        Focus::WorkspaceList => Focus::Main,
        Focus::Main => Focus::Detail,
        Focus::Detail => Focus::Composer,
        Focus::Composer => Focus::WorkspaceList,
    }
}

pub fn prev_focus(current: &Focus) -> Focus {
    match current {
        Focus::WorkspaceList => Focus::Composer,
        Focus::Main => Focus::WorkspaceList,
        Focus::Detail => Focus::Main,
        Focus::Composer => Focus::Detail,
    }
}
