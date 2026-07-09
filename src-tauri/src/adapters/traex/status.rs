use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use crate::adapters::hook_command::{hook_path_from_single_quoted_command, is_executable_file};
use crate::adapters::traex::hook_config::is_tokenfire_hook;
use crate::adapters::traex::resolver::TraexPaths;

pub const HOOK_SMOKE_FRESHNESS_WINDOW: Duration = Duration::minutes(30);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraexStatus {
    pub hook_installed: bool,
    pub hook_executable_exists: bool,
    pub hook_last_seen_at: Option<DateTime<Utc>>,
    pub hook_smoke_test_passed: bool,
    pub last_hook_error: Option<String>,
    pub sessions_readable: bool,
    pub archived_sessions_readable: bool,
}

#[derive(Debug, Clone)]
pub struct TraexStatusSource {
    pub config_path: PathBuf,
    pub hook_log: PathBuf,
    pub paths: TraexPaths,
}

impl TraexStatusSource {
    pub fn new(config_path: PathBuf, hook_log: PathBuf, paths: TraexPaths) -> Self {
        Self {
            config_path,
            hook_log,
            paths,
        }
    }

    pub fn collect(&self) -> TraexStatus {
        collect_traex_status(&self.config_path, &self.hook_log, &self.paths)
    }
}

pub fn collect_traex_status(
    config_path: &Path,
    hook_log: &Path,
    paths: &TraexPaths,
) -> TraexStatus {
    let command = tokenfire_command_from_config(config_path);
    let hook_path = command
        .as_deref()
        .and_then(hook_path_from_single_quoted_command);
    let config_modified_at = fs::metadata(config_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from);
    let hook_last_seen_at = read_last_hook_seen(hook_log, hook_path.as_deref(), config_modified_at);
    let hook_smoke_test_passed = hook_last_seen_at.is_some_and(|last_seen_at| {
        let age = Utc::now().signed_duration_since(last_seen_at);
        age >= Duration::zero() && age <= HOOK_SMOKE_FRESHNESS_WINDOW
    });

    TraexStatus {
        hook_installed: command.is_some(),
        hook_executable_exists: hook_path
            .as_ref()
            .is_some_and(|path| is_executable_file(path)),
        hook_last_seen_at,
        hook_smoke_test_passed,
        last_hook_error: read_last_hook_error(hook_log),
        sessions_readable: fs::read_dir(&paths.sessions_dir).is_ok(),
        archived_sessions_readable: fs::read_dir(&paths.archived_sessions_dir).is_ok(),
    }
}

fn tokenfire_command_from_config(config_path: &Path) -> Option<String> {
    let body = fs::read_to_string(config_path).ok()?;
    let doc = body.parse::<DocumentMut>().ok()?;
    let array = doc
        .get("Stop")
        .and_then(stop_hooks_array)
        .or_else(|| legacy_stop_hooks_array(&doc))?;
    let command = array.iter().find_map(|table| {
        let command = table.get("command").and_then(Item::as_str)?;
        is_tokenfire_hook(command).then(|| command.to_string())
    });
    command
}

fn stop_hooks_array(stop: &Item) -> Option<&toml_edit::ArrayOfTables> {
    if let Some(stop) = stop.as_table() {
        return stop.get("hooks").and_then(Item::as_array_of_tables);
    }
    stop.as_array_of_tables()
        .and_then(|stops| stops.iter().find_map(|table| table.get("hooks")))
        .and_then(Item::as_array_of_tables)
}

fn legacy_stop_hooks_array(doc: &DocumentMut) -> Option<&toml_edit::ArrayOfTables> {
    let hooks = doc.get("hooks")?.as_table()?;
    stop_hooks_array(hooks.get("Stop")?)
}

fn read_last_hook_seen(
    hook_log: &Path,
    hook_path: Option<&Path>,
    config_modified_at: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    read_hook_event_time(hook_log, |value| {
        let event = value.get("event")?.as_str()?;
        if !matches!(event, "hook_forwarded" | "hook_received") {
            return None;
        }
        if let Some(expected_path) = hook_path {
            let logged_path = value.get("hook_path")?.as_str()?;
            if !same_hook_path(Path::new(logged_path), expected_path) {
                return None;
            }
        }
        let ts = value.get("ts")?.as_str()?;
        let seen_at = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
        if config_modified_at.is_some_and(|modified_at| seen_at < modified_at) {
            return None;
        }
        Some(seen_at)
    })
}

fn same_hook_path(logged_path: &Path, expected_path: &Path) -> bool {
    match (logged_path.canonicalize(), expected_path.canonicalize()) {
        (Ok(logged), Ok(expected)) => logged == expected,
        _ => logged_path == expected_path,
    }
}

fn read_last_hook_error(hook_log: &Path) -> Option<String> {
    let body = fs::read_to_string(hook_log).ok()?;
    body.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        let event = value.get("event")?.as_str()?;
        matches!(
            event,
            "hook_socket_unavailable" | "hook_malformed_payload" | "hook_internal_failure"
        )
        .then(|| event.to_string())
    })
}

fn read_hook_event_time(
    hook_log: &Path,
    predicate: impl Fn(&Value) -> Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let body = fs::read_to_string(hook_log).ok()?;
    body.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        predicate(&value)
    })
}
