use serde::{Deserialize, Serialize};

use crate::adapters::source::SourceStatus;
use crate::adapters::traex::status::TraexStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiStatus {
    Green,
    Yellow,
    Red,
}

pub fn ui_status(
    traex: &TraexStatus,
    socket_ok: bool,
    watcher_ok: bool,
    sqlite_ok: bool,
) -> UiStatus {
    ui_status_from_sources(
        &[SourceStatus::from_traex(traex)],
        socket_ok,
        watcher_ok,
        sqlite_ok,
    )
}

pub fn ui_status_from_sources(
    sources: &[SourceStatus],
    socket_ok: bool,
    watcher_ok: bool,
    sqlite_ok: bool,
) -> UiStatus {
    if !socket_ok || !watcher_ok || !sqlite_ok {
        return UiStatus::Red;
    }
    if sources
        .iter()
        .filter(|source| source_participates_in_health(source))
        .any(|source| {
            !source.sessions_readable
                || !source.archived_sessions_readable
                || !source.hook_installed
                || !source.hook_executable_exists
                || !source.hook_smoke_test_passed
        })
    {
        return UiStatus::Yellow;
    }
    UiStatus::Green
}

fn source_participates_in_health(source: &SourceStatus) -> bool {
    source.enabled
}

pub fn status_label(status: UiStatus) -> &'static str {
    match status {
        UiStatus::Green => "正常",
        UiStatus::Yellow => "需处理",
        UiStatus::Red => "异常",
    }
}
