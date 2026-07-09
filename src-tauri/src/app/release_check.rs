use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Runtime};

use crate::app::build_identity::BuildIdentity;
use crate::app::logging::{LogFile, RuntimeLogSinks};

pub const GITHUB_LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/qieqie7/token-fire/releases/latest";
pub const GITHUB_RELEASES_URL: &str = "https://github.com/qieqie7/token-fire/releases";
pub const RELEASE_UPDATE_CHANGED_EVENT: &str = "release_update_changed";
const SUCCESS_CHECK_TTL_HOURS: i64 = 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseCheckFailureReason {
    Network,
    RateLimited,
    HttpStatus,
    InvalidResponse,
    InvalidVersion,
    NonStableRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReleaseUpdateStatus {
    Unknown {
        checked_at: Option<DateTime<Utc>>,
    },
    Checking {
        checked_at: Option<DateTime<Utc>>,
    },
    UpToDate {
        checked_at: DateTime<Utc>,
        current_version: String,
        latest_version: String,
    },
    UpdateAvailable {
        checked_at: DateTime<Utc>,
        current_version: String,
        current_commit_short: Option<String>,
        latest_version: String,
        latest_tag: String,
    },
    Failed {
        checked_at: Option<DateTime<Utc>>,
        reason: ReleaseCheckFailureReason,
    },
}

impl Default for ReleaseUpdateStatus {
    fn default() -> Self {
        Self::Unknown { checked_at: None }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReleaseUpdateStateStore {
    status: Arc<Mutex<ReleaseUpdateStatus>>,
}

impl ReleaseUpdateStateStore {
    pub fn new(initial: ReleaseUpdateStatus) -> Self {
        Self {
            status: Arc::new(Mutex::new(initial)),
        }
    }

    pub fn get(&self) -> ReleaseUpdateStatus {
        self.status
            .lock()
            .expect("release update status lock")
            .clone()
    }

    pub fn set(&self, status: ReleaseUpdateStatus) {
        *self.status.lock().expect("release update status lock") = status;
    }
}

pub fn trusted_release_url_for_status(status: &ReleaseUpdateStatus) -> String {
    match status {
        ReleaseUpdateStatus::UpdateAvailable { latest_tag, .. }
            if stable_release_tag_for_url(latest_tag).is_some() =>
        {
            format!("{GITHUB_RELEASES_URL}/tag/{latest_tag}")
        }
        ReleaseUpdateStatus::Unknown { .. }
        | ReleaseUpdateStatus::Checking { .. }
        | ReleaseUpdateStatus::UpToDate { .. }
        | ReleaseUpdateStatus::UpdateAvailable { .. }
        | ReleaseUpdateStatus::Failed { .. } => GITHUB_RELEASES_URL.to_string(),
    }
}

pub fn parse_release_version(tag_name: &str) -> Result<Version, ReleaseCheckFailureReason> {
    let normalized = tag_name.trim().strip_prefix('v').unwrap_or(tag_name.trim());
    Version::parse(normalized).map_err(|_| ReleaseCheckFailureReason::InvalidVersion)
}

fn stable_release_tag_for_url(tag_name: &str) -> Option<&str> {
    if tag_name.is_empty() || tag_name.trim() != tag_name {
        return None;
    }
    let version = parse_release_version(tag_name).ok()?;
    if version.pre.is_empty() {
        Some(tag_name)
    } else {
        None
    }
}

pub fn evaluate_github_release(
    current: &BuildIdentity,
    release: &GithubRelease,
    checked_at: DateTime<Utc>,
) -> ReleaseUpdateStatus {
    let latest_version = match parse_release_version(&release.tag_name) {
        Ok(version) => version,
        Err(reason) => {
            return ReleaseUpdateStatus::Failed {
                checked_at: Some(checked_at),
                reason,
            };
        }
    };
    let latest_version_string = latest_version.to_string();
    let current_version = match Version::parse(current.version.trim()) {
        Ok(version) => version,
        Err(_) => {
            return ReleaseUpdateStatus::Failed {
                checked_at: Some(checked_at),
                reason: ReleaseCheckFailureReason::InvalidVersion,
            };
        }
    };

    if release.draft || release.prerelease || !latest_version.pre.is_empty() {
        return ReleaseUpdateStatus::Failed {
            checked_at: Some(checked_at),
            reason: ReleaseCheckFailureReason::NonStableRelease,
        };
    }

    if latest_version <= current_version {
        return ReleaseUpdateStatus::UpToDate {
            checked_at,
            current_version: current.version.clone(),
            latest_version: latest_version_string,
        };
    }

    ReleaseUpdateStatus::UpdateAvailable {
        checked_at,
        current_version: current.version.clone(),
        current_commit_short: current.git_commit_short.clone(),
        latest_version: latest_version_string,
        latest_tag: release.tag_name.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseCheckCache {
    pub status: ReleaseUpdateStatus,
    pub last_success_checked_at: Option<DateTime<Utc>>,
    pub checked_current_version: Option<String>,
    pub last_failure_checked_at: Option<DateTime<Utc>>,
    pub last_failure_reason: Option<ReleaseCheckFailureReason>,
}

impl ReleaseCheckCache {
    pub fn from_status(status: ReleaseUpdateStatus) -> Self {
        let (last_success_checked_at, checked_current_version) = match &status {
            ReleaseUpdateStatus::UpToDate {
                checked_at,
                current_version,
                ..
            }
            | ReleaseUpdateStatus::UpdateAvailable {
                checked_at,
                current_version,
                ..
            } => (Some(*checked_at), Some(current_version.clone())),
            ReleaseUpdateStatus::Unknown { .. }
            | ReleaseUpdateStatus::Checking { .. }
            | ReleaseUpdateStatus::Failed { .. } => (None, None),
        };
        Self {
            status,
            last_success_checked_at,
            checked_current_version,
            last_failure_checked_at: None,
            last_failure_reason: None,
        }
    }

    pub fn status_for_current_version(
        &self,
        current: &BuildIdentity,
    ) -> Option<ReleaseUpdateStatus> {
        if self.checked_current_version.as_deref() == Some(current.version.as_str()) {
            Some(self.status.clone())
        } else {
            None
        }
    }

    pub fn should_reuse_success(&self, current: &BuildIdentity, now: DateTime<Utc>) -> bool {
        if self.checked_current_version.as_deref() != Some(current.version.as_str()) {
            return false;
        }
        self.last_success_checked_at.is_some_and(|checked_at| {
            now - checked_at < chrono::Duration::hours(SUCCESS_CHECK_TTL_HOURS)
        })
    }

    pub fn with_failure(
        mut self,
        checked_at: DateTime<Utc>,
        reason: ReleaseCheckFailureReason,
    ) -> Self {
        self.last_failure_checked_at = Some(checked_at);
        self.last_failure_reason = Some(reason);
        self
    }
}

#[derive(Debug, Clone)]
pub struct ReleaseCheckStore {
    path: PathBuf,
}

impl ReleaseCheckStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> anyhow::Result<Option<ReleaseCheckCache>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let body = fs::read_to_string(&self.path)?;
        let cache = serde_json::from_str(&body)?;
        Ok(Some(cache))
    }

    pub fn write(&self, cache: &ReleaseCheckCache) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string(cache)?;
        fs::write(&self.path, body)?;
        Ok(())
    }
}

pub fn open_release_url(url: &str) -> anyhow::Result<()> {
    Command::new("/usr/bin/open").arg(url).spawn()?;
    Ok(())
}

pub trait ReleaseHttpClient: Clone + Send + Sync + 'static {
    fn latest_release(&self, api_url: &str) -> Result<GithubRelease, ReleaseCheckFailureReason>;
}

#[derive(Debug, Clone, Default)]
pub struct GithubReleaseHttpClient;

impl ReleaseHttpClient for GithubReleaseHttpClient {
    fn latest_release(&self, api_url: &str) -> Result<GithubRelease, ReleaseCheckFailureReason> {
        let output = Command::new("/usr/bin/curl")
            .arg("--silent")
            .arg("--show-error")
            .arg("--location")
            .arg("--connect-timeout")
            .arg("3")
            .arg("--max-time")
            .arg("8")
            .arg("--header")
            .arg("User-Agent: TokenFire")
            .arg("--header")
            .arg("Accept: application/vnd.github+json")
            .arg("--write-out")
            .arg("\n%{http_code}")
            .arg(api_url)
            .output()
            .map_err(|_| ReleaseCheckFailureReason::Network)?;
        if !output.status.success() {
            return Err(ReleaseCheckFailureReason::Network);
        }
        let raw = String::from_utf8(output.stdout)
            .map_err(|_| ReleaseCheckFailureReason::InvalidResponse)?;
        let (body, status_text) = raw
            .rsplit_once('\n')
            .ok_or(ReleaseCheckFailureReason::InvalidResponse)?;
        let status = status_text
            .parse::<u16>()
            .map_err(|_| ReleaseCheckFailureReason::InvalidResponse)?;
        if status == 403 || status == 429 {
            return Err(ReleaseCheckFailureReason::RateLimited);
        }
        if !(200..300).contains(&status) {
            return Err(ReleaseCheckFailureReason::HttpStatus);
        }

        serde_json::from_str::<GithubRelease>(body)
            .map_err(|_| ReleaseCheckFailureReason::InvalidResponse)
    }
}

#[derive(Clone)]
pub struct ReleaseChecker<C: ReleaseHttpClient> {
    store: ReleaseCheckStore,
    client: C,
    logs: RuntimeLogSinks,
}

impl<C: ReleaseHttpClient> ReleaseChecker<C> {
    pub fn new(store: ReleaseCheckStore, client: C, logs: RuntimeLogSinks) -> Self {
        Self {
            store,
            client,
            logs,
        }
    }

    pub fn cached_success_for(
        &self,
        current: &BuildIdentity,
        now: DateTime<Utc>,
    ) -> Option<ReleaseUpdateStatus> {
        let cache = self.store.read().ok().flatten()?;
        if cache.should_reuse_success(current, now) {
            Some(cache.status)
        } else {
            None
        }
    }

    pub fn check_once(&self, current: &BuildIdentity, now: DateTime<Utc>) -> ReleaseUpdateStatus {
        let existing_cache = self.store.read().ok().flatten();
        if let Some(cache) = &existing_cache {
            if cache.should_reuse_success(current, now) {
                return cache.status.clone();
            }
        }

        let status = match self.client.latest_release(GITHUB_LATEST_RELEASE_API_URL) {
            Ok(release) => evaluate_github_release(current, &release, now),
            Err(reason) => ReleaseUpdateStatus::Failed {
                checked_at: Some(now),
                reason,
            },
        };

        let cache = match (&status, existing_cache) {
            (ReleaseUpdateStatus::Failed { reason, .. }, Some(previous)) => {
                if let Some(previous_status) = previous.status_for_current_version(current) {
                    let failed_cache = previous.with_failure(now, reason.clone());
                    if let Err(error) = self.store.write(&failed_cache) {
                        self.logs.write(
                            LogFile::App,
                            "release_check",
                            "warn",
                            "release_check_cache_write_failed",
                            json!({ "error": error.to_string() }),
                        );
                    }
                    self.log_failure(reason);
                    return previous_status;
                }
                ReleaseCheckCache::from_status(status.clone()).with_failure(now, reason.clone())
            }
            _ => ReleaseCheckCache::from_status(status.clone()),
        };
        if let Err(error) = self.store.write(&cache) {
            self.logs.write(
                LogFile::App,
                "release_check",
                "warn",
                "release_check_cache_write_failed",
                json!({ "error": error.to_string() }),
            );
        }

        if let ReleaseUpdateStatus::Failed { reason, .. } = &status {
            self.log_failure(reason);
        }

        status
    }

    fn log_failure(&self, reason: &ReleaseCheckFailureReason) {
        self.logs.write(
            LogFile::App,
            "release_check",
            "warn",
            "release_check_failed",
            json!({ "reason": reason }),
        );
    }
}

pub fn start_release_check_on_startup<R, C>(
    app: AppHandle<R>,
    state: ReleaseUpdateStateStore,
    checker: ReleaseChecker<C>,
    current: BuildIdentity,
) where
    R: Runtime,
    C: ReleaseHttpClient,
{
    if let Some(cached_status) = checker.cached_success_for(&current, Utc::now()) {
        state.set(cached_status);
        return;
    }

    let previous_checked_at = match state.get() {
        ReleaseUpdateStatus::UpToDate { checked_at, .. }
        | ReleaseUpdateStatus::UpdateAvailable { checked_at, .. } => Some(checked_at),
        ReleaseUpdateStatus::Unknown { checked_at }
        | ReleaseUpdateStatus::Checking { checked_at }
        | ReleaseUpdateStatus::Failed { checked_at, .. } => checked_at,
    };
    state.set(ReleaseUpdateStatus::Checking {
        checked_at: previous_checked_at,
    });

    std::thread::spawn(move || {
        let status = checker.check_once(&current, Utc::now());
        state.set(status.clone());
        let _ = app.emit(RELEASE_UPDATE_CHANGED_EVENT, status);
    });
}
