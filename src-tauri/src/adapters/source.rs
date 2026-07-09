use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenSourceKind {
    Traex,
    Codex,
    Claude,
    Cursor,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kind_supports_claude_and_cursor() {
        assert_eq!(TokenSourceKind::Claude.as_str(), "claude");
        assert_eq!(TokenSourceKind::Cursor.as_str(), "cursor");
        assert_eq!(
            TokenSourceKind::Claude.adapter_version(),
            "claude-transcript-v1"
        );
        assert_eq!(
            TokenSourceKind::Cursor.adapter_version(),
            "cursor-storage-estimate-v1"
        );
    }
}

impl TokenSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenSourceKind::Traex => "traex",
            TokenSourceKind::Codex => "codex",
            TokenSourceKind::Claude => "claude",
            TokenSourceKind::Cursor => "cursor",
        }
    }

    pub fn adapter_version(self) -> &'static str {
        match self {
            TokenSourceKind::Traex => "traex-jsonl-v1",
            TokenSourceKind::Codex => "codex-jsonl-v1",
            TokenSourceKind::Claude => "claude-transcript-v1",
            TokenSourceKind::Cursor => "cursor-storage-estimate-v1",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            TokenSourceKind::Traex => "TraeX",
            TokenSourceKind::Codex => "Codex",
            TokenSourceKind::Claude => "Claude",
            TokenSourceKind::Cursor => "Cursor",
        }
    }

    pub fn all_menu_sources() -> [TokenSourceKind; 4] {
        [
            TokenSourceKind::Traex,
            TokenSourceKind::Codex,
            TokenSourceKind::Claude,
            TokenSourceKind::Cursor,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceContext {
    pub source: TokenSourceKind,
    pub adapter_version: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHookStatus {
    pub source: TokenSourceKind,
    pub hook_registered: bool,
    pub hook_executable_exists: bool,
    pub config_detected: bool,
    pub config_error: Option<String>,
}

impl SourceContext {
    pub fn new(source: TokenSourceKind) -> Self {
        Self {
            source,
            adapter_version: source.adapter_version(),
        }
    }

    pub fn traex() -> Self {
        Self::new(TokenSourceKind::Traex)
    }

    pub fn codex() -> Self {
        Self::new(TokenSourceKind::Codex)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePaths {
    pub kind: TokenSourceKind,
    pub sessions_dir: PathBuf,
    pub archived_sessions_dir: PathBuf,
}

impl SourcePaths {
    pub fn new(
        kind: TokenSourceKind,
        sessions_dir: PathBuf,
        archived_sessions_dir: PathBuf,
    ) -> Self {
        Self {
            kind,
            sessions_dir,
            archived_sessions_dir,
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        let Ok(candidate) = path.canonicalize() else {
            return false;
        };
        [&self.sessions_dir, &self.archived_sessions_dir]
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| candidate.starts_with(root))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRegistry {
    sources: Vec<SourcePaths>,
}

impl SourceRegistry {
    pub fn new(sources: Vec<SourcePaths>) -> Self {
        Self { sources }
    }

    pub fn active_sources(&self) -> Vec<SourcePaths> {
        let mut active = Vec::new();
        for source in &self.sources {
            let overlaps_existing = active
                .iter()
                .any(|existing: &SourcePaths| same_or_overlapping_roots(existing, source));
            if !overlaps_existing {
                active.push(source.clone());
            }
        }
        active
    }

    pub fn source_paths(&self, kind: TokenSourceKind) -> Option<&SourcePaths> {
        self.sources.iter().find(|source| source.kind == kind)
    }
}

fn same_or_overlapping_roots(left: &SourcePaths, right: &SourcePaths) -> bool {
    roots_overlap(&left.sessions_dir, &right.sessions_dir)
        || roots_overlap(&left.archived_sessions_dir, &right.archived_sessions_dir)
}

fn roots_overlap(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right || left.starts_with(&right) || right.starts_with(&left)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStatus {
    pub source: TokenSourceKind,
    pub enabled: bool,
    pub detected: bool,
    pub hook_installed: bool,
    pub hook_executable_exists: bool,
    pub hook_smoke_test_passed: bool,
    pub sessions_readable: bool,
    pub archived_sessions_readable: bool,
    pub last_hook_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_hook_error: Option<String>,
}

impl SourceStatus {
    pub fn from_traex(status: &crate::adapters::traex::status::TraexStatus) -> Self {
        Self {
            source: TokenSourceKind::Traex,
            enabled: true,
            detected: true,
            hook_installed: status.hook_installed,
            hook_executable_exists: status.hook_executable_exists,
            hook_smoke_test_passed: status.hook_smoke_test_passed,
            sessions_readable: status.sessions_readable,
            archived_sessions_readable: status.archived_sessions_readable,
            last_hook_seen_at: status.hook_last_seen_at,
            last_hook_error: status.last_hook_error.clone(),
        }
    }
}
