use crate::app::widget_events::UsageFactsInvalidatedEvent;

pub fn notify_usage_facts_invalidated<E>(
    payload: &UsageFactsInvalidatedEvent,
    mut emit_events: impl FnMut(&UsageFactsInvalidatedEvent),
    mut refresh_surfaces: impl FnMut() -> Result<(), E>,
) {
    emit_events(payload);
    let _ = refresh_surfaces();
}
