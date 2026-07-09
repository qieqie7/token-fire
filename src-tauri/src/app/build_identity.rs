use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app::logging::write_jsonl_event;
use crate::app::paths::RuntimePaths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub version: String,
    pub git_commit: Option<String>,
    pub git_commit_short: Option<String>,
    pub build_time: Option<String>,
    pub dirty: bool,
}

pub fn current_build_identity() -> BuildIdentity {
    BuildIdentity {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_commit: non_empty(option_env!("TOKEN_FIRE_GIT_COMMIT")),
        git_commit_short: non_empty(option_env!("TOKEN_FIRE_GIT_COMMIT_SHORT")),
        build_time: non_empty(option_env!("TOKEN_FIRE_BUILD_TIME")),
        dirty: option_env!("TOKEN_FIRE_GIT_DIRTY").unwrap_or("true") == "true",
    }
}

pub fn fields_with_build_identity(fields: Value, identity: &BuildIdentity) -> Value {
    let mut object = match fields {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    object.insert(
        "version".to_string(),
        Value::String(identity.version.clone()),
    );
    object.insert(
        "git_commit".to_string(),
        identity
            .git_commit
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    object.insert(
        "git_commit_short".to_string(),
        identity
            .git_commit_short
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    object.insert(
        "build_time".to_string(),
        identity
            .build_time
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    object.insert("dirty".to_string(), Value::Bool(identity.dirty));
    Value::Object(object)
}

pub fn log_app_started(paths: &RuntimePaths, identity: &BuildIdentity) -> anyhow::Result<()> {
    write_jsonl_event(
        &paths.app_log,
        "app",
        "info",
        "app_started",
        fields_with_build_identity(serde_json::json!({}), identity),
    )
}

pub fn print_version_json(identity: &BuildIdentity) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(identity)?);
    Ok(())
}

pub fn has_version_json_arg() -> bool {
    std::env::args().any(|arg| arg == "--version-json")
}

fn non_empty(value: Option<&'static str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "unknown")
        .map(ToString::to_string)
}
