use std::fs;
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

use crate::adapters::hook_command::{
    hook_path_from_single_quoted_command, is_executable_file,
    is_tokenfire_owned_command_for_source, tokenfire_hook_command,
};
use crate::adapters::source::{SourceHookStatus, TokenSourceKind};

static BACKUP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookInstallResult {
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct HookConfigManager {
    config_path: PathBuf,
    backups_dir: PathBuf,
}

impl HookConfigManager {
    pub fn new(config_path: PathBuf, backups_dir: PathBuf) -> Self {
        Self {
            config_path,
            backups_dir,
        }
    }

    pub fn new_for_default_config(backups_dir: PathBuf) -> anyhow::Result<Self> {
        Ok(Self::new(default_config_path()?, backups_dir))
    }

    pub fn install(&self, hook_path: &Path) -> anyhow::Result<HookInstallResult> {
        let original = self.read_config()?;
        let mut doc = original.parse::<DocumentMut>()?;
        let desired_command = tokenfire_hook_command(hook_path, TokenSourceKind::Traex);
        if existing_tokenfire_commands(&doc) == [desired_command.as_str()] {
            return Ok(HookInstallResult { changed: false });
        }
        remove_tokenfire_hooks(&mut doc);
        append_stop_hook(&mut doc, &desired_command)?;
        self.backup(&original)?;
        self.atomic_write(doc.to_string().as_bytes())?;
        Ok(HookInstallResult { changed: true })
    }

    pub fn uninstall(&self) -> anyhow::Result<HookInstallResult> {
        let original = self.read_config()?;
        let mut doc = original.parse::<DocumentMut>()?;
        let changed = remove_tokenfire_hooks(&mut doc);
        if !changed {
            return Ok(HookInstallResult { changed: false });
        }
        self.backup(&original)?;
        self.atomic_write(doc.to_string().as_bytes())?;
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
            source: TokenSourceKind::Traex,
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
            .join(format!("traecli.toml.{timestamp}.{sequence}"));
        fs::write(backup, original)?;
        Ok(())
    }

    fn atomic_write(&self, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.config_path.with_extension("toml.tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(tmp, &self.config_path)?;
        Ok(())
    }
}

pub fn default_config_path() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory not found"))?;
    Ok(home.join(".trae").join("traecli.toml"))
}

pub fn is_tokenfire_hook(command: &str) -> bool {
    is_tokenfire_owned_command_for_source(command, TokenSourceKind::Traex)
}

fn append_stop_hook(doc: &mut DocumentMut, command: &str) -> anyhow::Result<()> {
    if let Some(hooks) = doc.get("hooks") {
        if !hooks.is_table() {
            bail!("expected hooks to be a table");
        }
    }
    let _ = remove_legacy_tokenfire_hooks(doc);
    if !doc.as_table().contains_key("Stop") {
        doc["Stop"] = Item::Table(Table::new());
    }
    let array = stop_hooks_array_mut(&mut doc["Stop"])?;
    let mut table = Table::new();
    table["command"] = value(command);
    table["type"] = value("command");
    table["timeout"] = value(5);
    array.push(table);
    Ok(())
}

fn stop_hooks_array_mut(stop: &mut Item) -> anyhow::Result<&mut ArrayOfTables> {
    if stop.is_table() {
        let stop = stop
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("expected hooks.Stop to be a table"))?;
        if !stop.contains_key("hooks") {
            stop["hooks"] = Item::ArrayOfTables(ArrayOfTables::new());
        }
        return stop["hooks"]
            .as_array_of_tables_mut()
            .ok_or_else(|| anyhow::anyhow!("expected hooks.Stop.hooks to be an array of tables"));
    }
    if stop.is_array_of_tables() {
        let stops = stop
            .as_array_of_tables_mut()
            .ok_or_else(|| anyhow::anyhow!("expected hooks.Stop to be an array of tables"))?;
        if stops.is_empty() {
            stops.push(Table::new());
        }
        let first = stops
            .get_mut(0)
            .ok_or_else(|| anyhow::anyhow!("expected hooks.Stop to contain a table"))?;
        if !first.contains_key("hooks") {
            first["hooks"] = Item::ArrayOfTables(ArrayOfTables::new());
        }
        return first["hooks"]
            .as_array_of_tables_mut()
            .ok_or_else(|| anyhow::anyhow!("expected hooks.Stop.hooks to be an array of tables"));
    }
    bail!("expected hooks.Stop to be a table or array of tables")
}

fn remove_tokenfire_hooks(doc: &mut DocumentMut) -> bool {
    let changed_stop = doc
        .get_mut("Stop")
        .and_then(first_stop_hooks_array_mut)
        .is_some_and(remove_tokenfire_from_array);
    let changed_legacy = remove_legacy_tokenfire_hooks(doc);
    changed_stop || changed_legacy
}

fn existing_tokenfire_commands(doc: &DocumentMut) -> Vec<&str> {
    let mut commands = Vec::new();
    if let Some(array) = doc.get("Stop").and_then(first_stop_hooks_array) {
        commands.extend(tokenfire_commands(array));
    }
    commands
}

fn tokenfire_command_from_config(config_path: &Path) -> anyhow::Result<Option<String>> {
    let body = match fs::read_to_string(config_path) {
        Ok(body) => body,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let doc = body
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    let array = doc
        .get("Stop")
        .and_then(first_stop_hooks_array)
        .or_else(|| legacy_stop_hooks_array(&doc));
    let command = array.and_then(|array| {
        array.iter().find_map(|table| {
            let command = table.get("command").and_then(Item::as_str)?;
            is_tokenfire_hook(command).then(|| command.to_string())
        })
    });
    Ok(command)
}

fn first_stop_hooks_array(stop: &Item) -> Option<&ArrayOfTables> {
    if let Some(stop) = stop.as_table() {
        return stop.get("hooks").and_then(Item::as_array_of_tables);
    }
    stop.as_array_of_tables()
        .and_then(|stops| stops.iter().find_map(|table| table.get("hooks")))
        .and_then(Item::as_array_of_tables)
}

fn first_stop_hooks_array_mut(stop: &mut Item) -> Option<&mut ArrayOfTables> {
    if stop.is_table() {
        let stop = stop.as_table_mut()?;
        return stop.get_mut("hooks").and_then(Item::as_array_of_tables_mut);
    }
    if stop.is_array_of_tables() {
        return stop
            .as_array_of_tables_mut()
            .and_then(|stops| stops.iter_mut().find_map(|table| table.get_mut("hooks")))
            .and_then(Item::as_array_of_tables_mut);
    }
    None
}

fn legacy_stop_hooks_array(doc: &DocumentMut) -> Option<&ArrayOfTables> {
    let hooks = doc.get("hooks")?.as_table()?;
    first_stop_hooks_array(hooks.get("Stop")?)
}

fn tokenfire_commands(array: &ArrayOfTables) -> impl Iterator<Item = &str> {
    array
        .iter()
        .filter_map(|table| table.get("command").and_then(Item::as_str))
        .filter(|command| is_tokenfire_hook(command))
}

fn remove_tokenfire_from_array(array: &mut ArrayOfTables) -> bool {
    let original_len = array.len();
    let retained = array
        .iter()
        .filter(|table| {
            let command = table
                .get("command")
                .and_then(Item::as_str)
                .unwrap_or_default();
            !is_tokenfire_hook(command)
        })
        .cloned()
        .collect::<Vec<_>>();
    array.clear();
    for table in retained {
        array.push(table);
    }
    array.len() != original_len
}

fn remove_legacy_tokenfire_hooks(doc: &mut DocumentMut) -> bool {
    let Some(hooks) = doc.get_mut("hooks").and_then(Item::as_table_mut) else {
        return false;
    };
    let Some(stop) = hooks.get_mut("Stop") else {
        return false;
    };
    let Some(array) = first_stop_hooks_array_mut(stop) else {
        return false;
    };
    remove_tokenfire_from_array(array)
}
