use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(debug_assertions)]
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde_json::{json, Value};

use crate::adapters::hook_command::{
    hook_path_from_single_quoted_command, is_executable_file,
    is_tokenfire_owned_command_for_source, tokenfire_hook_command,
};
use crate::adapters::source::{SourceHookStatus, TokenSourceKind};
use crate::adapters::traex::hook_config::HookInstallResult;

const CLAUDE_EVENTS: [&str; 3] = ["Stop", "StopFailure", "SubagentStop"];

static BACKUP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(debug_assertions)]
type BeforeWriteHook = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct ClaudeHookConfigManager {
    config_path: PathBuf,
    backups_dir: PathBuf,
    #[cfg(debug_assertions)]
    before_write_hook: Option<BeforeWriteHook>,
}

impl std::fmt::Debug for ClaudeHookConfigManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeHookConfigManager")
            .field("config_path", &self.config_path)
            .field("backups_dir", &self.backups_dir)
            .finish_non_exhaustive()
    }
}

impl ClaudeHookConfigManager {
    pub fn new(config_path: PathBuf, backups_dir: PathBuf) -> Self {
        Self {
            config_path,
            backups_dir,
            #[cfg(debug_assertions)]
            before_write_hook: None,
        }
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn new_with_before_write_hook_for_test<F>(
        config_path: PathBuf,
        backups_dir: PathBuf,
        before_write_hook: F,
    ) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            config_path,
            backups_dir,
            before_write_hook: Some(Arc::new(before_write_hook)),
        }
    }

    pub fn new_for_default_config(backups_dir: PathBuf) -> anyhow::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory not found"))?;
        Ok(Self::new(
            home.join(".claude").join("settings.json"),
            backups_dir,
        ))
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn install(&self, hook_path: &Path) -> anyhow::Result<HookInstallResult> {
        let _lock = self.acquire_write_lock()?;
        let original = self.read_config()?;
        let mut doc = parse_or_default(&original)?;
        let desired = tokenfire_hook_command(hook_path, TokenSourceKind::Claude);
        if desired_commands_installed(&doc, &desired) {
            return Ok(HookInstallResult { changed: false });
        }
        remove_tokenfire_hooks(&mut doc);
        append_claude_hooks(&mut doc, &desired)?;
        let updated = serde_json::to_vec_pretty(&doc)?;
        self.atomic_write_checked(original.as_bytes(), &updated)?;
        Ok(HookInstallResult { changed: true })
    }

    pub fn uninstall(&self) -> anyhow::Result<HookInstallResult> {
        let _lock = self.acquire_write_lock()?;
        let original = self.read_config()?;
        let mut doc = parse_or_default(&original)?;
        let changed = remove_tokenfire_hooks(&mut doc);
        if !changed {
            return Ok(HookInstallResult { changed: false });
        }
        let updated = serde_json::to_vec_pretty(&doc)?;
        self.atomic_write_checked(original.as_bytes(), &updated)?;
        Ok(HookInstallResult { changed })
    }

    pub fn status(&self) -> anyhow::Result<SourceHookStatus> {
        let command = tokenfire_command_from_config(&self.config_path)?;
        let hook_path = command
            .as_deref()
            .and_then(hook_path_from_single_quoted_command);
        let hook_executable_exists = hook_path
            .as_ref()
            .is_some_and(|path| is_executable_file(path));
        Ok(SourceHookStatus {
            source: TokenSourceKind::Claude,
            hook_registered: command.is_some(),
            hook_executable_exists,
            config_detected: self.config_path.exists(),
            config_error: None,
        })
    }

    fn read_config(&self) -> anyhow::Result<String> {
        match fs::read_to_string(&self.config_path) {
            Ok(content) => Ok(content),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(String::new()),
            Err(error) => {
                Err(error).with_context(|| format!("failed to read {}", self.config_path.display()))
            }
        }
    }

    fn backup(&self, original: &str) -> anyhow::Result<()> {
        fs::create_dir_all(&self.backups_dir)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_nanos();
        let sequence = BACKUP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let backup = self
            .backups_dir
            .join(format!("claude-settings.json.{timestamp}.{sequence}"));
        fs::write(backup, original)?;
        Ok(())
    }

    fn acquire_write_lock(&self) -> anyhow::Result<ConfigWriteLock> {
        let lock_path = self.config_path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let started = SystemTime::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    writeln!(file, "pid={}", std::process::id())?;
                    return Ok(ConfigWriteLock { path: lock_path });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if started.elapsed().unwrap_or_default() > Duration::from_secs(5) {
                        anyhow::bail!("settings.json lock is busy; retry install");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to create {}", lock_path.display()));
                }
            }
        }
    }

    fn atomic_write_checked(&self, original: &[u8], updated: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(debug_assertions)]
        {
            if let Some(before_write_hook) = &self.before_write_hook {
                before_write_hook();
            }
        }
        self.ensure_current_matches(original)?;
        let tmp = self.unique_temp_path()?;
        if let Err(error) = self.backup(std::str::from_utf8(original)?) {
            return Err(error);
        }
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            file.write_all(updated)?;
            file.sync_all()?;
        }
        self.ensure_current_matches(original).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        fs::rename(tmp, &self.config_path)?;
        Ok(())
    }

    fn unique_temp_path(&self) -> anyhow::Result<PathBuf> {
        let parent = self
            .config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = self
            .config_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid settings.json file name"))?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Ok(parent.join(format!(
            "{name}.{}.{}.{}.tmp",
            std::process::id(),
            timestamp,
            sequence
        )))
    }

    fn ensure_current_matches(&self, original: &[u8]) -> anyhow::Result<()> {
        match fs::read(&self.config_path) {
            Ok(current) if current == original => Ok(()),
            Ok(_) => anyhow::bail!("settings.json changed during write; retry install"),
            Err(error) if error.kind() == ErrorKind::NotFound && original.is_empty() => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                anyhow::bail!("settings.json changed during write; retry install")
            }
            Err(error) => {
                Err(error).with_context(|| format!("failed to read {}", self.config_path.display()))
            }
        }
    }
}

struct ConfigWriteLock {
    path: PathBuf,
}

impl Drop for ConfigWriteLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn parse_or_default(original: &str) -> anyhow::Result<Value> {
    if original.trim().is_empty() {
        return Ok(json!({ "hooks": {} }));
    }
    Ok(serde_json::from_str(original)?)
}

fn append_claude_hooks(doc: &mut Value, command: &str) -> anyhow::Result<()> {
    let root = doc
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("expected settings.json root to be an object"))?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("expected hooks to be an object"))?;
    for event in CLAUDE_EVENTS {
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        let groups = groups
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("expected hooks.{event} to be an array"))?;
        groups.push(json!({
            "hooks": [
                {
                    "type": "command",
                    "command": command
                }
            ]
        }));
    }
    Ok(())
}

fn remove_tokenfire_hooks(doc: &mut Value) -> bool {
    let Some(hooks) = doc.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    for event in CLAUDE_EVENTS {
        let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
            continue;
        };
        let mut remove_group = vec![false; groups.len()];
        for (index, group) in groups.iter_mut().enumerate() {
            let Some(commands) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            let before = commands.len();
            commands.retain(|hook| !is_tokenfire_command(hook));
            changed |= commands.len() != before;
            remove_group[index] =
                commands.len() != before && is_tokenfire_created_empty_group(group);
        }
        let before = groups.len();
        let mut index = 0;
        groups.retain(|_| {
            let keep = !remove_group[index];
            index += 1;
            keep
        });
        changed |= groups.len() != before;
    }
    changed
}

fn is_tokenfire_created_empty_group(group: &Value) -> bool {
    let Some(object) = group.as_object() else {
        return false;
    };
    object.len() == 1
        && object
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
}

fn desired_commands_installed(doc: &Value, desired: &str) -> bool {
    CLAUDE_EVENTS
        .iter()
        .all(|event| event_tokenfire_commands(doc, event) == [desired])
}

fn event_tokenfire_commands<'a>(doc: &'a Value, event: &str) -> Vec<&'a str> {
    doc.pointer(&format!("/hooks/{event}"))
        .into_iter()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter(|hook| is_tokenfire_command(hook))
        .filter_map(|hook| hook.get("command").and_then(Value::as_str))
        .collect()
}

fn tokenfire_command_from_config(config_path: &Path) -> anyhow::Result<Option<String>> {
    let body = match fs::read_to_string(config_path) {
        Ok(body) => body,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let doc = serde_json::from_str::<Value>(&body)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    Ok(CLAUDE_EVENTS
        .iter()
        .filter_map(|event| doc.pointer(&format!("/hooks/{event}")))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .find(|hook| is_tokenfire_command(hook))
        .and_then(|hook| hook.get("command").and_then(Value::as_str))
        .map(str::to_string))
}

fn is_tokenfire_command(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "command")
        && value
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                is_tokenfire_owned_command_for_source(command, TokenSourceKind::Claude)
            })
}
