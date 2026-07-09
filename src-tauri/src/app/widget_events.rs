use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

pub const USAGE_FACTS_INVALIDATED_EVENT: &str = "usage_facts_invalidated";
pub const PROFILE_SUMMARY_CHANGED_EVENT: &str = "profile_summary_changed";
pub const PROFILE_WINDOW_FOCUSED_EVENT: &str = "profile_window_focused";
pub const WIDGET_STATE_CHANGED_EVENT: &str = "widget_state_changed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetStateChangedEvent {
    pub state_revision: i64,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub inserted: usize,
}

pub type UsageFactsInvalidatedEvent = WidgetStateChangedEvent;

pub fn usage_fact_invalidation_event_names() -> [&'static str; 3] {
    [
        USAGE_FACTS_INVALIDATED_EVENT,
        PROFILE_SUMMARY_CHANGED_EVENT,
        WIDGET_STATE_CHANGED_EVENT,
    ]
}

pub fn emit_usage_fact_invalidation_events_with(
    payload: &UsageFactsInvalidatedEvent,
    mut emit: impl FnMut(&'static str, &UsageFactsInvalidatedEvent),
) {
    for event_name in usage_fact_invalidation_event_names() {
        emit(event_name, payload);
    }
}

#[derive(Clone)]
pub struct WidgetEventEmitter {
    emit: Arc<dyn Fn(UsageFactsInvalidatedEvent) + Send + Sync>,
}

pub fn emit_usage_fact_invalidation_events<R: Runtime>(
    app: &AppHandle<R>,
    payload: &UsageFactsInvalidatedEvent,
) {
    emit_usage_fact_invalidation_events_with(payload, |event_name, event_payload| {
        let _ = app.emit(event_name, event_payload.clone());
    });
}

impl WidgetEventEmitter {
    pub fn noop() -> Self {
        Self::from_fn(|_| {})
    }

    pub fn from_fn(emit: impl Fn(UsageFactsInvalidatedEvent) + Send + Sync + 'static) -> Self {
        Self {
            emit: Arc::new(emit),
        }
    }

    pub fn from_app<R: Runtime>(app: AppHandle<R>) -> Self {
        Self::from_fn(move |payload| emit_usage_fact_invalidation_events(&app, &payload))
    }

    pub fn emit_usage_facts_invalidated(&self, payload: UsageFactsInvalidatedEvent) {
        (self.emit)(payload);
    }

    pub fn emit_widget_state_changed(&self, payload: UsageFactsInvalidatedEvent) {
        self.emit_usage_facts_invalidated(payload);
    }
}
