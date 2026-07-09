use std::path::{Path, PathBuf};

use chrono::{Days, Local};
use walkdir::WalkDir;

use crate::adapters::source::{SourcePaths, TokenSourceKind};
use crate::adapters::HookMetadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraexPaths {
    pub sessions_dir: PathBuf,
    pub archived_sessions_dir: PathBuf,
}

impl TraexPaths {
    pub fn default_for_home() -> anyhow::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory not found"))?;
        Ok(Self {
            sessions_dir: home.join(".trae").join("cli").join("sessions"),
            archived_sessions_dir: home.join(".trae").join("cli").join("archived_sessions"),
        })
    }
}

impl From<TraexPaths> for SourcePaths {
    fn from(paths: TraexPaths) -> Self {
        SourcePaths::new(
            TokenSourceKind::Traex,
            paths.sessions_dir,
            paths.archived_sessions_dir,
        )
    }
}

impl From<&TraexPaths> for SourcePaths {
    fn from(paths: &TraexPaths) -> Self {
        SourcePaths::new(
            TokenSourceKind::Traex,
            paths.sessions_dir.clone(),
            paths.archived_sessions_dir.clone(),
        )
    }
}

pub fn resolve_transcript(
    paths: &TraexPaths,
    metadata: &HookMetadata,
) -> anyhow::Result<Option<PathBuf>> {
    resolve_transcript_for_source(&SourcePaths::from(paths), metadata)
}

pub fn resolve_transcript_for_source(
    paths: &SourcePaths,
    metadata: &HookMetadata,
) -> anyhow::Result<Option<PathBuf>> {
    if let Some(path) = metadata.transcript_path.as_ref().map(PathBuf::from) {
        if is_readable_jsonl_for_source(paths, &path) {
            return Ok(Some(path));
        }
    }

    let Some(session_id) = metadata.session_id.as_deref() else {
        return Ok(None);
    };

    for root in recent_session_roots(&paths.sessions_dir) {
        if let Some(path) = newest_matching_jsonl(&root, session_id, 3) {
            return Ok(Some(path));
        }
    }

    if let Some(path) = newest_matching_jsonl(&paths.archived_sessions_dir, session_id, 6) {
        return Ok(Some(path));
    }

    Ok(None)
}

pub fn is_allowed_transcript_candidate(paths: &TraexPaths, path: &Path) -> bool {
    is_allowed_transcript_candidate_for_source(&SourcePaths::from(paths), path)
}

pub fn is_allowed_transcript_candidate_for_source(paths: &SourcePaths, path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("jsonl")
        && is_allowed_subtree(path)
        && is_under_configured_source_root(paths, path)
}

fn recent_session_roots(root: &Path) -> Vec<PathBuf> {
    let today = Local::now().date_naive();
    [0_u64, 1, 2]
        .iter()
        .filter_map(|days_ago| today.checked_sub_days(Days::new(*days_ago)))
        .map(|day| {
            root.join(day.format("%Y").to_string())
                .join(day.format("%m").to_string())
                .join(day.format("%d").to_string())
        })
        .filter(|path| path.exists())
        .collect()
}

fn newest_matching_jsonl(root: &Path, session_id: &str, max_depth: usize) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }

    let mut matches = WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            let path = entry.path();
            is_allowed_subtree(path)
                && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
                && path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.contains(session_id))
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.into_path()))
        })
        .collect::<Vec<_>>();

    matches.sort_by_key(|(modified, _)| *modified);
    matches.pop().map(|(_, path)| path)
}

fn is_readable_jsonl_for_source(paths: &SourcePaths, path: &Path) -> bool {
    is_allowed_transcript_candidate_for_source(paths, path) && std::fs::File::open(path).is_ok()
}

fn is_allowed_subtree(path: &Path) -> bool {
    !path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|value| value == "tool-results" || value.ends_with(".artifacts"))
    })
}

fn is_under_configured_source_root(paths: &SourcePaths, path: &Path) -> bool {
    let Ok(candidate) = path.canonicalize() else {
        return false;
    };

    [&paths.sessions_dir, &paths.archived_sessions_dir]
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| candidate.starts_with(root))
}
