use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::adapters::codex::hook_config::is_tokenfire_command;
use crate::adapters::hook_command::{hook_path_from_single_quoted_command, is_executable_file};
use crate::adapters::source::{SourcePaths, SourceStatus, TokenSourceKind};
use crate::adapters::traex::status::HOOK_SMOKE_FRESHNESS_WINDOW;

#[derive(Debug, Clone)]
pub struct CodexStatusSource {
    pub config_path: PathBuf,
    pub hook_log: PathBuf,
    pub paths: SourcePaths,
}

impl CodexStatusSource {
    pub fn new(config_path: PathBuf, paths: SourcePaths) -> Self {
        Self {
            config_path,
            hook_log: PathBuf::new(),
            paths,
        }
    }

    pub fn new_with_hook_log(config_path: PathBuf, hook_log: PathBuf, paths: SourcePaths) -> Self {
        Self {
            config_path,
            hook_log,
            paths,
        }
    }

    pub fn collect(&self) -> SourceStatus {
        let detected = self.paths.sessions_dir.exists()
            || self.paths.archived_sessions_dir.exists()
            || self.config_path.exists();
        let command = codex_tokenfire_hook_command(&self.config_path);
        let hook_path = command
            .as_deref()
            .and_then(hook_path_from_single_quoted_command);
        let config_modified_at = fs::metadata(&self.config_path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .map(DateTime::<Utc>::from);
        let last_hook_seen_at =
            read_last_codex_hook_seen(&self.hook_log, hook_path.as_deref(), config_modified_at);
        let hook_executable_exists = hook_path
            .as_ref()
            .is_some_and(|path| is_executable_file(path));
        let hook_smoke_test_passed = hook_executable_exists
            && last_hook_seen_at.is_some_and(|last_seen_at| {
                let age = Utc::now().signed_duration_since(last_seen_at);
                age >= Duration::zero() && age <= HOOK_SMOKE_FRESHNESS_WINDOW
            });
        SourceStatus {
            source: TokenSourceKind::Codex,
            enabled: detected,
            detected,
            hook_installed: command.is_some(),
            hook_executable_exists,
            hook_smoke_test_passed,
            sessions_readable: fs::read_dir(&self.paths.sessions_dir).is_ok(),
            archived_sessions_readable: fs::read_dir(&self.paths.archived_sessions_dir).is_ok(),
            last_hook_seen_at,
            last_hook_error: read_last_codex_hook_error(&self.hook_log),
        }
    }
}

fn codex_tokenfire_hook_command(config_path: &Path) -> Option<String> {
    let Ok(body) = fs::read_to_string(config_path) else {
        return None;
    };
    let Ok(doc) = serde_json::from_str::<Value>(&body) else {
        return None;
    };
    doc.pointer("/hooks/Stop")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("hooks").and_then(Value::as_array))
        .flatten()
        .find(|hook| is_tokenfire_command(hook))
        .and_then(|hook| hook.get("command").and_then(Value::as_str))
        .map(str::to_string)
}

fn read_last_codex_hook_seen(
    hook_log: &Path,
    hook_path: Option<&Path>,
    config_modified_at: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    read_hook_event_time(hook_log, |value| {
        let event = value.get("event")?.as_str()?;
        if !matches!(event, "hook_forwarded" | "hook_received") {
            return None;
        }
        if value.get("source")?.as_str()? != "codex" {
            return None;
        }
        if let Some(expected_path) = hook_path {
            let logged_path = Path::new(value.get("hook_path")?.as_str()?);
            if !same_hook_path(logged_path, expected_path) {
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

fn read_last_codex_hook_error(hook_log: &Path) -> Option<String> {
    let body = fs::read_to_string(hook_log).ok()?;
    body.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        if value.get("source").and_then(Value::as_str) != Some("codex") {
            return None;
        }
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

fn same_hook_path(logged_path: &Path, expected_path: &Path) -> bool {
    match (logged_path.canonicalize(), expected_path.canonicalize()) {
        (Ok(logged), Ok(expected)) => logged == expected,
        _ => logged_path == expected_path,
    }
}
