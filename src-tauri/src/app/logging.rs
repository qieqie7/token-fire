use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Map, Value};

use crate::app::paths::RuntimePaths;

const RESERVED_LOG_KEYS: &[&str] = &["ts", "level", "component", "event"];
pub(crate) const FORBIDDEN_LOG_KEYS: &[&str] = &[
    "prompt",
    "response",
    "tool_arguments",
    "command_output",
    "raw_line",
    "file_content",
];

pub fn append_hook_log(
    paths: &RuntimePaths,
    level: &str,
    event: &str,
    fields: Value,
) -> anyhow::Result<()> {
    write_jsonl_event(&paths.hook_log, "hook", level, event, fields)
}

#[derive(Debug, Clone)]
pub struct RuntimeLogSinks {
    pub paths: RuntimePaths,
    pub debug_gate: DebugLogGate,
}

impl RuntimeLogSinks {
    pub fn new(paths: RuntimePaths, debug_gate: DebugLogGate) -> Self {
        Self { paths, debug_gate }
    }

    pub fn write(&self, file: LogFile, component: &str, level: &str, event: &str, fields: Value) {
        let path = match file {
            LogFile::App => &self.paths.app_log,
            LogFile::Parser => &self.paths.parser_log,
            LogFile::Db => &self.paths.db_log,
        };
        let _ =
            write_jsonl_event_if_enabled(path, component, level, event, fields, &self.debug_gate);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LogFile {
    App,
    Parser,
    Db,
}

#[derive(Debug, Clone)]
pub struct RuntimeLogger {
    paths: RuntimePaths,
    gate: DebugLogGate,
}

impl RuntimeLogger {
    pub fn new(paths: RuntimePaths, gate: DebugLogGate) -> Self {
        Self { paths, gate }
    }

    pub fn gate(&self) -> DebugLogGate {
        self.gate.clone()
    }
}

#[derive(Debug, Clone, Default)]
pub struct DebugLogGate {
    debug_until: Arc<Mutex<Option<DateTime<Utc>>>>,
}

impl DebugLogGate {
    pub fn enable_debug_for_30_minutes(&self, now: DateTime<Utc>) {
        *self.debug_until.lock().expect("debug log gate lock") = Some(now + Duration::minutes(30));
    }

    pub fn should_write(&self, level: &str, now: DateTime<Utc>) -> bool {
        if level != "debug" {
            return true;
        }
        self.debug_until
            .lock()
            .expect("debug log gate lock")
            .is_some_and(|until| now < until)
    }
}

pub fn write_jsonl_event_if_enabled(
    path: &Path,
    component: &str,
    level: &str,
    event: &str,
    fields: Value,
    gate: &DebugLogGate,
) -> anyhow::Result<()> {
    if gate.should_write(level, Utc::now()) {
        write_jsonl_event(path, component, level, event, fields)?;
    }
    Ok(())
}

pub fn append_app_log(
    logger: &RuntimeLogger,
    level: &str,
    event: &str,
    fields: Value,
) -> anyhow::Result<()> {
    write_jsonl_event_if_enabled(
        &logger.paths.app_log,
        "app",
        level,
        event,
        fields,
        &logger.gate,
    )
}

pub fn write_jsonl_event(
    path: &Path,
    component: &str,
    level: &str,
    event: &str,
    fields: Value,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "level": level,
        "component": component,
        "event": event
    });
    if let Value::Object(extra) = fields {
        let object = line.as_object_mut().expect("json object");
        for (key, value) in extra {
            if RESERVED_LOG_KEYS.contains(&key.as_str())
                || FORBIDDEN_LOG_KEYS.contains(&key.as_str())
            {
                continue;
            }
            object.insert(key, sanitize_value(value));
        }
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&line)?)?;
    Ok(())
}

pub(crate) fn sanitize_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_value).collect()),
        Value::Object(object) => {
            let mut sanitized = Map::new();
            for (key, value) in object {
                if FORBIDDEN_LOG_KEYS.contains(&key.as_str()) {
                    continue;
                }
                sanitized.insert(key, sanitize_value(value));
            }
            Value::Object(sanitized)
        }
        scalar => scalar,
    }
}
