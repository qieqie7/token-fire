use chrono::{TimeZone, Utc};
use token_fire::app::build_identity::BuildIdentity;
use token_fire::app::release_check::{
    evaluate_github_release, parse_release_version, trusted_release_url_for_status, GithubRelease,
    ReleaseCheckFailureReason, ReleaseUpdateStateStore, ReleaseUpdateStatus, GITHUB_RELEASES_URL,
};

fn identity(version: &str) -> BuildIdentity {
    BuildIdentity {
        version: version.to_string(),
        git_commit: Some("2b67267abcdef".to_string()),
        git_commit_short: Some("2b67267".to_string()),
        build_time: Some("unix:123".to_string()),
        dirty: false,
    }
}

fn release(tag: &str) -> GithubRelease {
    GithubRelease {
        tag_name: tag.to_string(),
        draft: false,
        prerelease: false,
    }
}

#[test]
fn release_version_parser_accepts_v_prefixed_and_plain_semver_tags() {
    assert_eq!(
        parse_release_version("v0.1.1").unwrap().to_string(),
        "0.1.1"
    );
    assert_eq!(parse_release_version("0.1.1").unwrap().to_string(), "0.1.1");
}

#[test]
fn release_version_parser_rejects_invalid_tags() {
    assert_eq!(
        parse_release_version("release-2026-07-09").unwrap_err(),
        ReleaseCheckFailureReason::InvalidVersion
    );
}

#[test]
fn release_evaluation_reports_update_available_for_newer_stable_release() {
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();

    let status = evaluate_github_release(&identity("0.1.0"), &release("v0.1.1"), checked_at);

    assert_eq!(
        status,
        ReleaseUpdateStatus::UpdateAvailable {
            checked_at,
            current_version: "0.1.0".to_string(),
            current_commit_short: Some("2b67267".to_string()),
            latest_version: "0.1.1".to_string(),
            latest_tag: "v0.1.1".to_string(),
        }
    );
}

#[test]
fn release_evaluation_keeps_equal_or_lower_versions_up_to_date() {
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();

    assert_eq!(
        evaluate_github_release(&identity("0.1.0"), &release("v0.1.0"), checked_at),
        ReleaseUpdateStatus::UpToDate {
            checked_at,
            current_version: "0.1.0".to_string(),
            latest_version: "0.1.0".to_string(),
        }
    );

    assert_eq!(
        evaluate_github_release(&identity("0.1.1"), &release("v0.1.0"), checked_at),
        ReleaseUpdateStatus::UpToDate {
            checked_at,
            current_version: "0.1.1".to_string(),
            latest_version: "0.1.0".to_string(),
        }
    );
}

#[test]
fn release_evaluation_rejects_non_stable_releases_without_success_status() {
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let mut draft = release("v0.2.0");
    draft.draft = true;
    let mut prerelease = release("v0.2.0");
    prerelease.prerelease = true;
    let semver_prerelease = release("v0.2.0-beta.1");

    assert_eq!(
        evaluate_github_release(&identity("0.1.0"), &draft, checked_at),
        ReleaseUpdateStatus::Failed {
            checked_at: Some(checked_at),
            reason: ReleaseCheckFailureReason::NonStableRelease,
        }
    );

    assert_eq!(
        evaluate_github_release(&identity("0.1.0"), &prerelease, checked_at),
        ReleaseUpdateStatus::Failed {
            checked_at: Some(checked_at),
            reason: ReleaseCheckFailureReason::NonStableRelease,
        }
    );

    assert_eq!(
        evaluate_github_release(&identity("0.1.0"), &semver_prerelease, checked_at),
        ReleaseUpdateStatus::Failed {
            checked_at: Some(checked_at),
            reason: ReleaseCheckFailureReason::NonStableRelease,
        }
    );
}

use tempfile::tempdir;
use token_fire::app::release_check::{ReleaseCheckCache, ReleaseCheckStore};

#[test]
fn release_check_cache_round_trips_minimal_status_without_raw_response() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join("release-check.json");
    let store = ReleaseCheckStore::new(cache_path.clone());
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let status = ReleaseUpdateStatus::UpdateAvailable {
        checked_at,
        current_version: "0.1.0".to_string(),
        current_commit_short: Some("2b67267".to_string()),
        latest_version: "0.1.1".to_string(),
        latest_tag: "v0.1.1".to_string(),
    };

    store
        .write(&ReleaseCheckCache::from_status(status.clone()))
        .unwrap();

    let raw = std::fs::read_to_string(&cache_path).unwrap();
    assert!(raw.contains(r#""state":"update_available""#));
    assert!(raw.contains(r#""latest_version":"0.1.1""#));
    assert!(!raw.contains("assets"));
    assert!(!raw.contains("body"));
    assert!(!raw.contains("raw_response"));

    let restored = store.read().unwrap().unwrap();
    assert_eq!(restored.status, status);
    assert_eq!(restored.last_success_checked_at, Some(checked_at));
    assert_eq!(restored.checked_current_version.as_deref(), Some("0.1.0"));
}

#[test]
fn release_check_cache_reuses_only_recent_successful_checks_for_current_version() {
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let cache = ReleaseCheckCache::from_status(ReleaseUpdateStatus::UpToDate {
        checked_at,
        current_version: "0.1.0".to_string(),
        latest_version: "0.1.0".to_string(),
    });

    assert!(
        cache.should_reuse_success(&identity("0.1.0"), checked_at + chrono::Duration::hours(2))
    );
    assert!(
        !cache.should_reuse_success(&identity("0.1.0"), checked_at + chrono::Duration::hours(3))
    );
    assert!(
        !cache.should_reuse_success(&identity("0.1.0"), checked_at + chrono::Duration::hours(4))
    );
    assert!(
        !cache.should_reuse_success(&identity("0.1.1"), checked_at + chrono::Duration::hours(1))
    );

    let failed = ReleaseCheckCache::from_status(ReleaseUpdateStatus::Failed {
        checked_at: Some(checked_at),
        reason: ReleaseCheckFailureReason::Network,
    });
    assert!(!failed.should_reuse_success(
        &identity("0.1.0"),
        checked_at + chrono::Duration::minutes(1)
    ));
}

#[test]
fn release_check_cache_keeps_last_success_when_failure_is_recorded() {
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let failed_at = checked_at + chrono::Duration::hours(25);
    let cache = ReleaseCheckCache::from_status(ReleaseUpdateStatus::UpdateAvailable {
        checked_at,
        current_version: "0.1.0".to_string(),
        current_commit_short: Some("2b67267".to_string()),
        latest_version: "0.1.1".to_string(),
        latest_tag: "v0.1.1".to_string(),
    })
    .with_failure(failed_at, ReleaseCheckFailureReason::Network);

    assert_eq!(cache.last_success_checked_at, Some(checked_at));
    assert_eq!(cache.last_failure_checked_at, Some(failed_at));
    assert_eq!(
        cache.last_failure_reason,
        Some(ReleaseCheckFailureReason::Network)
    );
    assert!(matches!(
        cache.status_for_current_version(&identity("0.1.0")),
        Some(ReleaseUpdateStatus::UpdateAvailable { .. })
    ));
}

use std::sync::{Arc, Mutex};
use token_fire::app::logging::{DebugLogGate, RuntimeLogSinks};
use token_fire::app::paths::RuntimePaths;
use token_fire::app::release_check::{
    GithubReleaseHttpClient, ReleaseChecker, ReleaseHttpClient, GITHUB_LATEST_RELEASE_API_URL,
};

fn runtime_paths(home: &std::path::Path) -> RuntimePaths {
    let run_dir = home.join("run");
    let logs_dir = home.join("logs");
    RuntimePaths {
        database: home.join("token-fire.sqlite"),
        socket: run_dir.join("token-fire.sock"),
        app_log: logs_dir.join("app.log"),
        hook_log: logs_dir.join("hook.log"),
        parser_log: logs_dir.join("parser.log"),
        db_log: logs_dir.join("db.log"),
        backups_dir: home.join("backups"),
        debug_bundles_dir: home.join("debug-bundles"),
        home: home.to_path_buf(),
        run_dir,
        logs_dir,
    }
}

#[derive(Clone)]
struct FakeReleaseClient {
    calls: Arc<Mutex<usize>>,
    result: Result<GithubRelease, ReleaseCheckFailureReason>,
}

impl FakeReleaseClient {
    fn new(result: Result<GithubRelease, ReleaseCheckFailureReason>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(0)),
            result,
        }
    }

    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl ReleaseHttpClient for FakeReleaseClient {
    fn latest_release(&self, api_url: &str) -> Result<GithubRelease, ReleaseCheckFailureReason> {
        assert_eq!(api_url, GITHUB_LATEST_RELEASE_API_URL);
        *self.calls.lock().unwrap() += 1;
        self.result.clone()
    }
}

#[test]
fn release_checker_reuses_recent_success_cache_without_network() {
    let dir = tempdir().unwrap();
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let store = ReleaseCheckStore::new(dir.path().join("release-check.json"));
    store
        .write(&ReleaseCheckCache::from_status(
            ReleaseUpdateStatus::UpToDate {
                checked_at,
                current_version: "0.1.0".to_string(),
                latest_version: "0.1.0".to_string(),
            },
        ))
        .unwrap();
    let client = FakeReleaseClient::new(Ok(release("v0.1.1")));
    let checker = ReleaseChecker::new(
        store,
        client.clone(),
        RuntimeLogSinks::new(runtime_paths(dir.path()), DebugLogGate::default()),
    );

    let current = identity("0.1.0");
    let status = checker.check_once(&current, checked_at + chrono::Duration::hours(1));

    assert_eq!(client.calls(), 0);
    assert!(matches!(status, ReleaseUpdateStatus::UpToDate { .. }));
}

#[test]
fn release_checker_refreshes_recent_cache_when_current_version_changed() {
    let dir = tempdir().unwrap();
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let store = ReleaseCheckStore::new(dir.path().join("release-check.json"));
    store
        .write(&ReleaseCheckCache::from_status(
            ReleaseUpdateStatus::UpdateAvailable {
                checked_at,
                current_version: "0.1.0".to_string(),
                current_commit_short: Some("2b67267".to_string()),
                latest_version: "0.1.1".to_string(),
                latest_tag: "v0.1.1".to_string(),
            },
        ))
        .unwrap();
    let client = FakeReleaseClient::new(Ok(release("v0.1.1")));
    let checker = ReleaseChecker::new(
        store,
        client.clone(),
        RuntimeLogSinks::new(runtime_paths(dir.path()), DebugLogGate::default()),
    );

    let status = checker.check_once(&identity("0.1.1"), checked_at + chrono::Duration::hours(1));

    assert_eq!(client.calls(), 1);
    assert!(matches!(status, ReleaseUpdateStatus::UpToDate { .. }));
}

#[test]
fn release_checker_refreshes_stale_cache_and_writes_success() {
    let dir = tempdir().unwrap();
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let store = ReleaseCheckStore::new(dir.path().join("release-check.json"));
    store
        .write(&ReleaseCheckCache::from_status(
            ReleaseUpdateStatus::UpToDate {
                checked_at,
                current_version: "0.1.0".to_string(),
                latest_version: "0.1.0".to_string(),
            },
        ))
        .unwrap();
    let client = FakeReleaseClient::new(Ok(release("v0.1.1")));
    let checker = ReleaseChecker::new(
        store.clone(),
        client.clone(),
        RuntimeLogSinks::new(runtime_paths(dir.path()), DebugLogGate::default()),
    );

    let status = checker.check_once(&identity("0.1.0"), checked_at + chrono::Duration::hours(25));

    assert_eq!(client.calls(), 1);
    assert!(matches!(
        status,
        ReleaseUpdateStatus::UpdateAvailable { .. }
    ));
    assert!(matches!(
        store.read().unwrap().unwrap().status,
        ReleaseUpdateStatus::UpdateAvailable { .. }
    ));
}

#[test]
fn release_checker_keeps_previous_success_visible_when_refresh_fails() {
    let dir = tempdir().unwrap();
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let failed_at = checked_at + chrono::Duration::hours(25);
    let store = ReleaseCheckStore::new(dir.path().join("release-check.json"));
    store
        .write(&ReleaseCheckCache::from_status(
            ReleaseUpdateStatus::UpdateAvailable {
                checked_at,
                current_version: "0.1.0".to_string(),
                current_commit_short: Some("2b67267".to_string()),
                latest_version: "0.1.1".to_string(),
                latest_tag: "v0.1.1".to_string(),
            },
        ))
        .unwrap();
    let client = FakeReleaseClient::new(Err(ReleaseCheckFailureReason::Network));
    let checker = ReleaseChecker::new(
        store.clone(),
        client,
        RuntimeLogSinks::new(runtime_paths(dir.path()), DebugLogGate::default()),
    );

    let status = checker.check_once(&identity("0.1.0"), failed_at);

    assert!(matches!(
        status,
        ReleaseUpdateStatus::UpdateAvailable { .. }
    ));
    let cache = store.read().unwrap().unwrap();
    assert_eq!(cache.last_success_checked_at, Some(checked_at));
    assert_eq!(cache.last_failure_checked_at, Some(failed_at));
    assert_eq!(
        cache.last_failure_reason,
        Some(ReleaseCheckFailureReason::Network)
    );
}

#[test]
fn release_checker_logs_network_failure_without_success_throttle() {
    let dir = tempdir().unwrap();
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    let store = ReleaseCheckStore::new(dir.path().join("release-check.json"));
    let paths = runtime_paths(dir.path());
    let client = FakeReleaseClient::new(Err(ReleaseCheckFailureReason::Network));
    let checker = ReleaseChecker::new(
        store.clone(),
        client,
        RuntimeLogSinks::new(paths.clone(), DebugLogGate::default()),
    );

    let status = checker.check_once(&identity("0.1.0"), checked_at);

    assert_eq!(
        status,
        ReleaseUpdateStatus::Failed {
            checked_at: Some(checked_at),
            reason: ReleaseCheckFailureReason::Network,
        }
    );
    assert_eq!(store.read().unwrap().unwrap().last_success_checked_at, None);
    let app_log = std::fs::read_to_string(paths.app_log).unwrap();
    assert!(app_log.contains(r#""component":"release_check""#));
    assert!(app_log.contains(r#""event":"release_check_failed""#));
}

#[test]
fn github_release_http_client_type_is_available_for_runtime_wiring() {
    let _client = GithubReleaseHttpClient::default();
}

#[test]
fn release_update_state_store_returns_latest_status_snapshot() {
    let store = ReleaseUpdateStateStore::default();
    assert_eq!(
        store.get(),
        ReleaseUpdateStatus::Unknown { checked_at: None }
    );

    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    store.set(ReleaseUpdateStatus::UpdateAvailable {
        checked_at,
        current_version: "0.1.0".to_string(),
        current_commit_short: Some("2b67267".to_string()),
        latest_version: "0.1.1".to_string(),
        latest_tag: "v0.1.1".to_string(),
    });

    assert!(matches!(
        store.get(),
        ReleaseUpdateStatus::UpdateAvailable { .. }
    ));
}

#[test]
fn trusted_release_url_for_status_derives_fixed_repo_tag_url_only_when_update_is_available() {
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();
    assert_eq!(
        trusted_release_url_for_status(&ReleaseUpdateStatus::UpdateAvailable {
            checked_at,
            current_version: "0.1.0".to_string(),
            current_commit_short: Some("2b67267".to_string()),
            latest_version: "0.1.1".to_string(),
            latest_tag: "v0.1.1".to_string(),
        }),
        "https://github.com/qieqie7/token-fire/releases/tag/v0.1.1"
    );

    assert_eq!(
        trusted_release_url_for_status(&ReleaseUpdateStatus::UpdateAvailable {
            checked_at,
            current_version: "0.1.0".to_string(),
            current_commit_short: Some("2b67267".to_string()),
            latest_version: "0.1.1".to_string(),
            latest_tag: "0.1.1".to_string(),
        }),
        "https://github.com/qieqie7/token-fire/releases/tag/0.1.1"
    );

    assert_eq!(
        trusted_release_url_for_status(&ReleaseUpdateStatus::UpToDate {
            checked_at,
            current_version: "0.1.0".to_string(),
            latest_version: "0.1.0".to_string(),
        }),
        GITHUB_RELEASES_URL
    );
}

#[test]
fn trusted_release_url_for_status_rejects_poisoned_update_tags() {
    let checked_at = Utc.with_ymd_and_hms(2026, 7, 9, 10, 0, 0).unwrap();

    for poisoned_tag in ["v0.1.1/../../x", "v0.1.1?x=1", "v0.1.1#x", " v0.1.1 "] {
        let url = trusted_release_url_for_status(&ReleaseUpdateStatus::UpdateAvailable {
            checked_at,
            current_version: "0.1.0".to_string(),
            current_commit_short: Some("2b67267".to_string()),
            latest_version: "0.1.1".to_string(),
            latest_tag: poisoned_tag.to_string(),
        });

        assert_eq!(url, GITHUB_RELEASES_URL);
        assert_ne!(url, format!("{GITHUB_RELEASES_URL}/tag/{poisoned_tag}"));
    }
}
