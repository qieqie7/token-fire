use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::adapters::source::SourceStatus;
use crate::adapters::traex::status::TraexStatus;
use crate::app::build_identity::current_build_identity;
use crate::app::logging::{sanitize_value, FORBIDDEN_LOG_KEYS};
use crate::app::paths::RuntimePaths;
use crate::core::usage_store::{RetentionPolicy, UsageStore};

static BUNDLE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeHealth {
    pub socket_ok: bool,
    pub watcher_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DebugBundleSourceSummary {
    pub source: String,
    pub enabled: bool,
    pub detected: bool,
    pub hook_install_status: String,
    pub hook_executable_status: String,
    pub hook_smoke_test_status: String,
    pub sessions_readable: bool,
    pub archived_sessions_readable: bool,
    pub last_hook_seen_at: Option<String>,
    pub last_hook_error: Option<String>,
}

impl Default for RuntimeHealth {
    fn default() -> Self {
        Self {
            socket_ok: false,
            watcher_ok: false,
        }
    }
}

pub fn create_debug_bundle(paths: &RuntimePaths, strict_privacy: bool) -> anyhow::Result<PathBuf> {
    create_debug_bundle_with_status(paths, strict_privacy, &TraexStatus::default())
}

pub fn create_debug_bundle_with_status(
    paths: &RuntimePaths,
    strict_privacy: bool,
    traex_status: &TraexStatus,
) -> anyhow::Result<PathBuf> {
    create_debug_bundle_with_status_and_runtime_health(
        paths,
        strict_privacy,
        traex_status,
        RuntimeHealth::default(),
    )
}

pub fn create_debug_bundle_with_status_and_runtime_health(
    paths: &RuntimePaths,
    strict_privacy: bool,
    traex_status: &TraexStatus,
    runtime_health: RuntimeHealth,
) -> anyhow::Result<PathBuf> {
    create_debug_bundle_with_source_statuses_and_runtime_health(
        paths,
        strict_privacy,
        &[SourceStatus::from_traex(traex_status)],
        runtime_health,
    )
}

pub fn create_debug_bundle_with_source_statuses_and_runtime_health(
    paths: &RuntimePaths,
    strict_privacy: bool,
    source_statuses: &[SourceStatus],
    runtime_health: RuntimeHealth,
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(&paths.debug_bundles_dir)?;
    let events = [
        &paths.app_log,
        &paths.hook_log,
        &paths.parser_log,
        &paths.db_log,
    ]
    .iter()
    .flat_map(|path| read_redacted_events(path, strict_privacy))
    .rev()
    .take(200)
    .collect::<Vec<_>>();

    let sqlite_metadata = sqlite_metadata(paths, strict_privacy);
    let retention = retention_metadata(paths);
    let recent_observations = UsageStore::open(&paths.database)
        .and_then(|store| store.recent_observation_metadata(20))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| redact_event(value, strict_privacy))
        .collect::<Vec<_>>();
    let sqlite_health_status = if UsageStore::open(&paths.database).is_ok() {
        "ok"
    } else {
        "error"
    };
    let bundle = json!({
        "created_at": chrono::Utc::now().to_rfc3339(),
        "build_identity": build_identity_diagnostic(&paths.hook_log),
        "events": events,
        "sqlite_metadata": sqlite_metadata,
        "retention": retention,
        "recent_observations": recent_observations,
        "sources": source_statuses_summary(source_statuses),
        "socket_status": if runtime_health.socket_ok { "running" } else { "unavailable" },
        "watcher_status": if runtime_health.watcher_ok { "running" } else { "unavailable" },
        "hook_install_status": merged_hook_install_status(source_statuses),
        "hook_executable_status": merged_hook_executable_status(source_statuses),
        "hook_smoke_test_status": merged_hook_smoke_test_status(source_statuses),
        "traex_directory_readability_status": traex_directory_readability_status(source_statuses),
        "source_directory_readability_status": {
            "sources": source_statuses.iter().map(source_directory_readability_status).collect::<Vec<_>>()
        },
        "sqlite_health_status": sqlite_health_status
    });
    let now = chrono::Utc::now();
    let counter = BUNDLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = paths.debug_bundles_dir.join(format!(
        "token-fire-debug.{}.{:09}.{}.json",
        now.format("%Y%m%d%H%M%S"),
        now.timestamp_subsec_nanos(),
        counter
    ));
    fs::write(&path, serde_json::to_string_pretty(&bundle)?)?;
    Ok(path)
}

fn hook_sidecar_identity(hook_log_path: &Path) -> Value {
    if let Some(identity) = recent_hook_log_identity(hook_log_path) {
        return identity;
    }
    adjacent_hook_sidecar_identity()
}

fn recent_hook_log_identity(path: &Path) -> Option<Value> {
    let body = fs::read_to_string(path).ok()?;
    body.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(build_identity_fields_from_value)
}

fn build_identity_fields_from_value(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    let version = identity_string_field(object, "version")?;
    let git_commit = identity_string_or_null_field(object, "git_commit")?;
    let git_commit_short = identity_string_or_null_field(object, "git_commit_short")?;
    let build_time = identity_string_or_null_field(object, "build_time")?;
    let dirty = object.get("dirty")?.as_bool()?;
    Some(json!({
        "version": version,
        "git_commit": git_commit,
        "git_commit_short": git_commit_short,
        "build_time": build_time,
        "dirty": dirty
    }))
}

fn identity_string_field(object: &Map<String, Value>, key: &str) -> Option<Value> {
    match object.get(key)? {
        Value::String(_) => object.get(key).cloned(),
        _ => None,
    }
}

fn identity_string_or_null_field(object: &Map<String, Value>, key: &str) -> Option<Value> {
    match object.get(key)? {
        Value::String(_) | Value::Null => object.get(key).cloned(),
        _ => None,
    }
}

fn adjacent_hook_sidecar_identity() -> Value {
    let hook_path = match std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("token-fire-hook")))
    {
        Some(path) if path.exists() => path,
        _ => {
            return json!({
                "status": "unavailable",
                "error_kind": "hook_executable_not_found"
            })
        }
    };

    match std::process::Command::new(&hook_path)
        .arg("--version-json")
        .output()
    {
        Ok(output) if output.status.success() => serde_json::from_slice::<Value>(&output.stdout)
            .ok()
            .and_then(build_identity_fields_from_value)
            .unwrap_or_else(
                || json!({ "status": "unavailable", "error_kind": "invalid_version_json" }),
            ),
        Ok(output) => {
            json!({ "status": "unavailable", "error_kind": format!("exit_status_{}", output.status) })
        }
        Err(error) => json!({
            "status": "unavailable",
            "error_kind": format!("io_{:?}", error.kind()).to_lowercase()
        }),
    }
}

fn build_identity_diagnostic(hook_log_path: &Path) -> Value {
    let app_runtime = serde_json::to_value(current_build_identity()).unwrap_or_else(|_| json!({}));
    let hook_sidecar = hook_sidecar_identity(hook_log_path);
    let mismatch = match (
        app_runtime.get("git_commit").and_then(Value::as_str),
        hook_sidecar.get("git_commit").and_then(Value::as_str),
    ) {
        (Some(app_commit), Some(hook_commit)) => Value::Bool(app_commit != hook_commit),
        _ => Value::Null,
    };
    json!({
        "app_runtime": app_runtime,
        "hook_sidecar": hook_sidecar,
        "mismatch": mismatch
    })
}

fn read_redacted_events(path: &Path, strict_privacy: bool) -> Vec<Value> {
    let Ok(body) = fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| redact_event(value, strict_privacy))
        .collect()
}

fn redact_event(value: Value, strict_privacy: bool) -> Option<Value> {
    let Value::Object(object) = value else {
        return None;
    };
    Some(Value::Object(redact_object(object, strict_privacy)))
}

fn redact_object(object: Map<String, Value>, strict_privacy: bool) -> Map<String, Value> {
    let mut redacted = Map::new();
    for (key, value) in object {
        match key.as_str() {
            key if FORBIDDEN_LOG_KEYS.contains(&key) || key == "tool_payload" => {}
            "source_record_id_confidence" if strict_privacy => {}
            "cwd" => {
                if !strict_privacy {
                    redacted.insert(key, redact_home_path(value));
                }
            }
            "source_path" => {
                redacted.insert(key, redact_source_path(value, strict_privacy));
            }
            "hook_path" => {
                redacted.insert(key, redact_hook_path_value(value));
            }
            _ => {
                redacted.insert(key, redact_value(value, strict_privacy));
            }
        }
    }
    redacted
}

fn redact_value(value: Value, strict_privacy: bool) -> Value {
    let value = sanitize_value(value);
    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| redact_value(value, strict_privacy))
                .collect(),
        ),
        Value::Object(object) => Value::Object(redact_object(object, strict_privacy)),
        scalar => scalar,
    }
}

fn redact_hook_path_value(value: Value) -> Value {
    let Some(path) = value.as_str() else {
        return redact_value(value, true);
    };
    Value::String(redact_hook_path(path))
}

fn redact_hook_path(value: &str) -> String {
    if value.contains("/Applications/TokenFire.app/") {
        "applications_bundle/token-fire-hook".to_string()
    } else if value.ends_with("token-fire-hook") {
        "dev_target/token-fire-hook".to_string()
    } else {
        "unknown/token-fire-hook".to_string()
    }
}

fn redact_home_path(value: Value) -> Value {
    let Some(path) = value.as_str() else {
        return value;
    };
    let Some(home) = dirs::home_dir() else {
        return Value::String(path.to_string());
    };
    let home = home.to_string_lossy();
    Value::String(path.replacen(home.as_ref(), "~", 1))
}

fn redact_source_path(value: Value, strict_privacy: bool) -> Value {
    let Some(path) = value.as_str() else {
        return value;
    };
    let basename = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path);
    if strict_privacy {
        Value::String(basename.to_string())
    } else {
        redact_home_path(Value::String(path.to_string()))
    }
}

pub fn debug_bundle_sources_summary(statuses: &[SourceStatus]) -> Vec<DebugBundleSourceSummary> {
    statuses
        .iter()
        .map(|status| DebugBundleSourceSummary {
            source: status.source.as_str().to_string(),
            enabled: status.enabled,
            detected: status.detected,
            hook_install_status: hook_install_status(status).to_string(),
            hook_executable_status: hook_executable_status(status).to_string(),
            hook_smoke_test_status: hook_smoke_test_status(status).to_string(),
            sessions_readable: status.sessions_readable,
            archived_sessions_readable: status.archived_sessions_readable,
            last_hook_seen_at: status.last_hook_seen_at.map(|seen_at| seen_at.to_rfc3339()),
            last_hook_error: status.last_hook_error.clone(),
        })
        .collect()
}

fn source_statuses_summary(statuses: &[SourceStatus]) -> Value {
    serde_json::to_value(debug_bundle_sources_summary(statuses))
        .unwrap_or_else(|_| Value::Array(vec![]))
}

fn hook_install_status(status: &SourceStatus) -> &'static str {
    if status.hook_installed {
        "installed"
    } else {
        "missing"
    }
}

fn hook_executable_status(status: &SourceStatus) -> &'static str {
    if status.hook_executable_exists {
        "exists"
    } else {
        "missing"
    }
}

fn hook_smoke_test_status(status: &SourceStatus) -> &'static str {
    if status.hook_smoke_test_passed {
        "passed"
    } else {
        "not_passed"
    }
}

fn source_directory_readability_status(status: &SourceStatus) -> Value {
    json!({
        "source": status.source.as_str(),
        "sessions_readable": status.sessions_readable,
        "archived_sessions_readable": status.archived_sessions_readable
    })
}

fn traex_directory_readability_status(statuses: &[SourceStatus]) -> Value {
    statuses
        .iter()
        .find(|status| status.source == crate::adapters::source::TokenSourceKind::Traex)
        .map(source_directory_readability_status)
        .unwrap_or_else(|| {
            json!({
                "sessions_readable": false,
                "archived_sessions_readable": false
            })
        })
}

fn merged_hook_install_status(statuses: &[SourceStatus]) -> &'static str {
    if statuses
        .iter()
        .filter(|status| status.enabled)
        .all(|status| status.hook_installed)
    {
        "installed"
    } else {
        "missing"
    }
}

fn merged_hook_executable_status(statuses: &[SourceStatus]) -> &'static str {
    if statuses
        .iter()
        .filter(|status| status.enabled)
        .all(|status| status.hook_executable_exists)
    {
        "exists"
    } else {
        "missing"
    }
}

fn merged_hook_smoke_test_status(statuses: &[SourceStatus]) -> &'static str {
    if statuses
        .iter()
        .filter(|status| status.enabled)
        .all(|status| status.hook_smoke_test_passed)
    {
        "passed"
    } else {
        "not_passed"
    }
}

fn sqlite_metadata(paths: &RuntimePaths, strict_privacy: bool) -> Value {
    if !paths.database.exists() {
        return json!({
            "status": "absent",
            "observations": []
        });
    }
    let Ok(conn) = Connection::open(&paths.database) else {
        return json!({
            "status": "unreadable",
            "observations": []
        });
    };
    let result = query_observation_metadata(&conn, strict_privacy);
    match result {
        Ok(observations) => json!({
            "status": "available",
            "observations": observations
        }),
        Err(error) if error.to_string().contains("no such table") => json!({
            "status": "table_absent",
            "observations": []
        }),
        Err(_) => json!({
            "status": "unreadable",
            "observations": []
        }),
    }
}

fn retention_metadata(paths: &RuntimePaths) -> Value {
    let default_policy = RetentionPolicy::default();
    if !paths.database.exists() {
        return json!({
            "policy_days": default_policy.observation_retention_days,
            "min_interval_hours": default_policy.min_interval_hours,
            "last_success_at": null,
            "last_deleted_observations": null,
            "last_failure_at": null,
            "last_error_kind": null
        });
    }
    match UsageStore::open(&paths.database)
        .and_then(|store| store.retention_diagnostics(default_policy))
    {
        Ok(diagnostics) => json!({
            "policy_days": diagnostics.policy_days,
            "min_interval_hours": diagnostics.min_interval_hours,
            "last_success_at": diagnostics.last_success_at,
            "last_deleted_observations": diagnostics.last_deleted_observations,
            "last_failure_at": diagnostics.last_failure_at,
            "last_error_kind": diagnostics.last_error_kind
        }),
        Err(_) => json!({
            "policy_days": default_policy.observation_retention_days,
            "min_interval_hours": default_policy.min_interval_hours,
            "last_success_at": null,
            "last_deleted_observations": null,
            "last_failure_at": null,
            "last_error_kind": "retention_metadata_unavailable"
        }),
    }
}

fn query_observation_metadata(
    conn: &Connection,
    strict_privacy: bool,
) -> anyhow::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        r#"
        select source, adapter_version, session_id, turn_id, turn_boundary_id,
               source_path, line_no, byte_offset, input_tokens, output_tokens,
               cached_input_tokens, cache_creation_input_tokens, reasoning_output_tokens,
               total_tokens, cumulative_total_tokens, model, observed_at, created_at
        from token_observations
        order by observed_at desc, id desc
        limit 50
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let source_path: Option<String> = row.get(5)?;
        let mut metadata = Map::new();
        metadata.insert("source".to_string(), json!(row.get::<_, String>(0)?));
        metadata.insert(
            "adapter_version".to_string(),
            json!(row.get::<_, String>(1)?),
        );
        metadata.insert(
            "session_id".to_string(),
            json!(row.get::<_, Option<String>>(2)?),
        );
        metadata.insert(
            "turn_id".to_string(),
            json!(row.get::<_, Option<String>>(3)?),
        );
        metadata.insert(
            "turn_boundary_id".to_string(),
            json!(row.get::<_, Option<String>>(4)?),
        );
        metadata.insert(
            "source_path".to_string(),
            source_path
                .map(|path| redact_source_path(Value::String(path), strict_privacy))
                .unwrap_or(Value::Null),
        );
        metadata.insert("line_no".to_string(), json!(row.get::<_, Option<i64>>(6)?));
        metadata.insert(
            "byte_offset".to_string(),
            json!(row.get::<_, Option<i64>>(7)?),
        );
        metadata.insert("input_tokens".to_string(), json!(row.get::<_, i64>(8)?));
        metadata.insert("output_tokens".to_string(), json!(row.get::<_, i64>(9)?));
        metadata.insert(
            "cached_input_tokens".to_string(),
            json!(row.get::<_, i64>(10)?),
        );
        metadata.insert(
            "cache_creation_input_tokens".to_string(),
            json!(row.get::<_, i64>(11)?),
        );
        metadata.insert(
            "reasoning_output_tokens".to_string(),
            json!(row.get::<_, i64>(12)?),
        );
        metadata.insert("total_tokens".to_string(), json!(row.get::<_, i64>(13)?));
        metadata.insert(
            "cumulative_total_tokens".to_string(),
            json!(row.get::<_, Option<i64>>(14)?),
        );
        metadata.insert(
            "model".to_string(),
            json!(row.get::<_, Option<String>>(15)?),
        );
        metadata.insert("observed_at".to_string(), json!(row.get::<_, String>(16)?));
        metadata.insert("created_at".to_string(), json!(row.get::<_, String>(17)?));
        Ok(Value::Object(metadata))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
