use crate::adapters::source::{SourcePaths, SourceRegistry, TokenSourceKind};
use crate::adapters::traex::resolver::TraexPaths;
use crate::adapters::HookMetadata;
use crate::app::ingest_scheduler::IngestScheduler;
use crate::app::runtime::{handle_runtime_event, RuntimeEvent};
use crate::app::tracking::TrackingGate;

pub fn handle_hook_metadata(
    source: TokenSourceKind,
    metadata: HookMetadata,
    registry: &SourceRegistry,
    scheduler: &IngestScheduler,
    tracking_gate: &TrackingGate,
) -> anyhow::Result<bool> {
    let report = handle_runtime_event(
        RuntimeEvent::Hook { source, metadata },
        registry,
        scheduler,
        tracking_gate,
    )?
    .is_some();
    Ok(report)
}

pub fn handle_traex_hook_metadata(
    metadata: HookMetadata,
    paths: &TraexPaths,
    scheduler: &IngestScheduler,
    tracking_gate: &TrackingGate,
) -> anyhow::Result<bool> {
    let registry = SourceRegistry::new(vec![SourcePaths::from(paths)]);
    handle_hook_metadata(
        TokenSourceKind::Traex,
        metadata,
        &registry,
        scheduler,
        tracking_gate,
    )
}
