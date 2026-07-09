use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::adapters::source::{SourcePaths, TokenSourceKind};
use crate::adapters::traex::resolver::{
    is_allowed_transcript_candidate, is_allowed_transcript_candidate_for_source, TraexPaths,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileEvent {
    pub source: TokenSourceKind,
    pub path: PathBuf,
}

pub fn watch_paths(
    paths: TraexPaths,
    sender: Sender<PathBuf>,
) -> notify::Result<RecommendedWatcher> {
    let event_paths = paths.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    for path in event.paths {
                        if is_watch_candidate(&event_paths, &path) {
                            let _ = sender.send(path);
                        }
                    }
                }
            }
        },
        Config::default(),
    )?;

    for root in watch_roots(&paths) {
        watcher.watch(&root, RecursiveMode::Recursive)?;
    }

    Ok(watcher)
}

pub fn watch_source_paths(
    paths: SourcePaths,
    sender: Sender<SourceFileEvent>,
) -> notify::Result<RecommendedWatcher> {
    let event_paths = paths.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    for path in event.paths {
                        if is_watch_candidate_for_source(&event_paths, &path) {
                            let _ = sender.send(SourceFileEvent {
                                source: event_paths.kind,
                                path,
                            });
                        }
                    }
                }
            }
        },
        Config::default(),
    )?;

    for root in watch_roots_for_source(&paths) {
        watcher.watch(&root, RecursiveMode::Recursive)?;
    }

    Ok(watcher)
}

pub fn watch_roots(paths: &TraexPaths) -> Vec<PathBuf> {
    watch_roots_for_source(&SourcePaths::from(paths))
}

pub fn watch_roots_for_source(paths: &SourcePaths) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for path in [&paths.sessions_dir, &paths.archived_sessions_dir] {
        let boundary = path.parent().unwrap_or(path);
        let Some(root) = nearest_existing_watch_root(path, boundary) else {
            continue;
        };
        if !roots.iter().any(|existing| existing == &root) {
            roots.push(root);
        }
    }
    roots
}

fn nearest_existing_watch_root(path: &Path, boundary: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if !candidate.starts_with(boundary) {
            return None;
        }
        if candidate.exists() {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
}

pub fn is_watch_candidate(paths: &TraexPaths, path: &std::path::Path) -> bool {
    is_allowed_transcript_candidate(paths, path)
}

pub fn is_watch_candidate_for_source(paths: &SourcePaths, path: &Path) -> bool {
    is_allowed_transcript_candidate_for_source(paths, path)
}
