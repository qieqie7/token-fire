use std::path::Path;

use crate::adapters::source::{SourcePaths, TokenSourceKind};

pub fn paths_for_home(home: &Path) -> SourcePaths {
    SourcePaths::new(
        TokenSourceKind::Codex,
        home.join(".codex").join("sessions"),
        home.join(".codex").join("archived_sessions"),
    )
}

pub fn default_paths() -> anyhow::Result<SourcePaths> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory not found"))?;
    Ok(paths_for_home(&home))
}
