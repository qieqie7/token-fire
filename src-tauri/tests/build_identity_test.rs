use assert_cmd::Command;
use serde_json::json;
use tempfile::tempdir;
use token_fire::app::build_identity::{
    current_build_identity, fields_with_build_identity, log_app_started, BuildIdentity,
};
use token_fire::app::paths::RuntimePaths;

fn paths(home: &std::path::Path) -> RuntimePaths {
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

#[test]
fn current_build_identity_uses_package_version() {
    let identity = current_build_identity();

    assert_eq!(identity.version, env!("CARGO_PKG_VERSION"));
    assert!(!identity.version.trim().is_empty());
    assert!(identity.build_time.is_some());
}

#[test]
fn build_identity_can_be_serialized_for_logs() {
    let identity = BuildIdentity {
        version: "0.1.1".to_string(),
        git_commit: Some("7e17eb0abcdef".to_string()),
        git_commit_short: Some("7e17eb0".to_string()),
        build_time: Some("unix:123".to_string()),
        dirty: false,
    };

    let fields = fields_with_build_identity(json!({ "event_source": "test" }), &identity);

    assert_eq!(fields["event_source"], "test");
    assert_eq!(fields["version"], "0.1.1");
    assert_eq!(fields["git_commit"], "7e17eb0abcdef");
    assert_eq!(fields["git_commit_short"], "7e17eb0");
    assert_eq!(fields["build_time"], "unix:123");
    assert_eq!(fields["dirty"], false);
}

#[test]
fn app_started_log_includes_build_identity() {
    let dir = tempdir().unwrap();
    let paths = paths(dir.path());
    let identity = BuildIdentity {
        version: "0.1.1".to_string(),
        git_commit: Some("7e17eb0abcdef".to_string()),
        git_commit_short: Some("7e17eb0".to_string()),
        build_time: Some("unix:123".to_string()),
        dirty: true,
    };

    log_app_started(&paths, &identity).unwrap();

    let body = std::fs::read_to_string(paths.app_log).unwrap();
    assert!(body.contains(r#""event":"app_started""#));
    assert!(body.contains(r#""version":"0.1.1""#));
    assert!(body.contains(r#""git_commit_short":"7e17eb0""#));
    assert!(body.contains(r#""dirty":true"#));
}

#[test]
fn token_fire_binary_prints_version_json_without_starting_gui() {
    let mut cmd = Command::cargo_bin("token-fire").unwrap();

    let output = cmd
        .arg("--version-json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert!(value.get("dirty").is_some());
    assert!(value.get("build_time").is_some());
}
