use std::fs;

use chrono::{Datelike, Local};
use tempfile::tempdir;
use token_fire::adapters::source::{SourcePaths, TokenSourceKind};
use token_fire::adapters::traex::resolver::{resolve_transcript, TraexPaths};
use token_fire::adapters::traex::watcher::is_watch_candidate;
use token_fire::adapters::HookMetadata;

#[test]
fn uses_readable_transcript_path_first() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let transcript = sessions.join("rollout-019-session.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let paths = TraexPaths {
        sessions_dir: sessions,
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };
    let metadata = HookMetadata {
        transcript_path: Some(transcript.to_string_lossy().to_string()),
        ..HookMetadata::default()
    };

    assert_eq!(
        resolve_transcript(&paths, &metadata).unwrap(),
        Some(transcript)
    );
}

#[test]
fn source_paths_detect_overlapping_roots_as_aliases() {
    let dir = tempdir().unwrap();
    let traex = SourcePaths::new(
        TokenSourceKind::Traex,
        dir.path().join("sessions"),
        dir.path().join("archived_sessions"),
    );
    let codex_alias = SourcePaths::new(
        TokenSourceKind::Codex,
        dir.path().join("sessions"),
        dir.path().join("archived_sessions"),
    );

    let registry =
        token_fire::adapters::source::SourceRegistry::new(vec![traex.clone(), codex_alias]);
    let active = registry.active_sources();

    assert_eq!(active, vec![traex]);
}

#[test]
fn codex_default_paths_use_codex_home_layout() {
    let home = tempdir().unwrap();
    let paths = token_fire::adapters::codex::paths::paths_for_home(home.path());

    assert_eq!(paths.kind, TokenSourceKind::Codex);
    assert_eq!(
        paths.sessions_dir,
        home.path().join(".codex").join("sessions")
    );
    assert_eq!(
        paths.archived_sessions_dir,
        home.path().join(".codex").join("archived_sessions")
    );
}

#[test]
fn rejects_direct_transcript_path_outside_configured_roots() {
    let dir = tempdir().unwrap();
    let outside = dir.path().join("outside/rollout-019-session.jsonl");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&outside, "{}\n").unwrap();
    let paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };
    let metadata = HookMetadata {
        transcript_path: Some(outside.to_string_lossy().to_string()),
        ..HookMetadata::default()
    };

    assert_eq!(resolve_transcript(&paths, &metadata).unwrap(), None);
}

#[test]
fn recursively_resolves_active_and_archived_session_files() {
    let dir = tempdir().unwrap();
    let today = Local::now();
    let active = dir
        .path()
        .join("sessions")
        .join(format!("{:04}", today.year()))
        .join(format!("{:02}", today.month()))
        .join(format!("{:02}", today.day()))
        .join("rollout-019-active.jsonl");
    let archived = dir
        .path()
        .join("archived_sessions/deep/rollout-019-archived.jsonl");
    fs::create_dir_all(active.parent().unwrap()).unwrap();
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::write(&active, "{}\n").unwrap();
    fs::write(&archived, "{}\n").unwrap();
    let paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };

    let active_metadata = HookMetadata {
        session_id: Some("019-active".to_string()),
        ..HookMetadata::default()
    };
    let archived_metadata = HookMetadata {
        session_id: Some("019-archived".to_string()),
        ..HookMetadata::default()
    };

    assert_eq!(
        resolve_transcript(&paths, &active_metadata).unwrap(),
        Some(active)
    );
    assert_eq!(
        resolve_transcript(&paths, &archived_metadata).unwrap(),
        Some(archived)
    );
}

#[test]
fn excludes_artifacts_tool_results_and_non_jsonl_files() {
    let dir = tempdir().unwrap();
    let excluded = dir
        .path()
        .join("sessions/2026/06/20/rollout-019-skip.artifacts/rollout-019-skip.jsonl");
    let tool_result = dir
        .path()
        .join("sessions/tool-results/rollout-019-skip.jsonl");
    let txt = dir.path().join("sessions/rollout-019-skip.txt");
    fs::create_dir_all(excluded.parent().unwrap()).unwrap();
    fs::create_dir_all(tool_result.parent().unwrap()).unwrap();
    fs::create_dir_all(txt.parent().unwrap()).unwrap();
    fs::write(&excluded, "{}\n").unwrap();
    fs::write(&tool_result, "{}\n").unwrap();
    fs::write(&txt, "{}\n").unwrap();
    let paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };
    let metadata = HookMetadata {
        session_id: Some("019-skip".to_string()),
        ..HookMetadata::default()
    };

    assert_eq!(resolve_transcript(&paths, &metadata).unwrap(), None);
}

#[test]
fn watcher_rejects_artifacts_tool_results_and_non_jsonl_paths() {
    let dir = tempdir().unwrap();
    let paths = TraexPaths {
        sessions_dir: dir.path().join("sessions"),
        archived_sessions_dir: dir.path().join("archived_sessions"),
    };
    let valid = paths.sessions_dir.join("2026/06/20/rollout-019-ok.jsonl");
    let artifact = paths
        .sessions_dir
        .join("2026/06/20/rollout-019-ok.artifacts/rollout-019-ok.jsonl");
    let tool_result = paths.sessions_dir.join("tool-results/rollout-019-ok.jsonl");
    let txt = paths.sessions_dir.join("rollout-019-ok.txt");
    for path in [&valid, &artifact, &tool_result, &txt] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "{}\n").unwrap();
    }

    assert!(is_watch_candidate(&paths, &valid));
    assert!(!is_watch_candidate(&paths, &artifact));
    assert!(!is_watch_candidate(&paths, &tool_result));
    assert!(!is_watch_candidate(&paths, &txt));
}
