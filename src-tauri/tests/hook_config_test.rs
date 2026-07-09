use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;

use tempfile::tempdir;
use token_fire::adapters::claude::hook_config::ClaudeHookConfigManager;
use token_fire::adapters::codex::hook_config::CodexHookConfigManager;
use token_fire::adapters::cursor::hook_config::CursorHookConfigManager;
use token_fire::adapters::hook_command::{
    is_tokenfire_owned_command_for_source, tokenfire_hook_command,
};
use token_fire::adapters::source::TokenSourceKind;
use token_fire::adapters::traex::hook_config::{is_tokenfire_hook, HookConfigManager};
use toml_edit::DocumentMut;

#[test]
fn hook_command_ownership_is_source_specific() {
    let claude_command = tokenfire_hook_command(
        Path::new("/Applications/TokenFire.app/Contents/MacOS/token-fire-hook"),
        TokenSourceKind::Claude,
    );
    let cursor_command = tokenfire_hook_command(
        Path::new("/Applications/TokenFire.app/Contents/MacOS/token-fire-hook"),
        TokenSourceKind::Cursor,
    );

    assert!(is_tokenfire_owned_command_for_source(
        &claude_command,
        TokenSourceKind::Claude
    ));
    assert!(!is_tokenfire_owned_command_for_source(
        &cursor_command,
        TokenSourceKind::Claude
    ));
    assert!(!is_tokenfire_owned_command_for_source(
        "external-tool --note 'token-fire-hook --owner token-fire --source claude'",
        TokenSourceKind::Claude
    ));
}

#[test]
fn hook_command_ownership_requires_tokenfire_hook_as_first_argv() {
    assert!(!is_tokenfire_owned_command_for_source(
        "echo '/tmp/token-fire-hook' --source claude --owner token-fire",
        TokenSourceKind::Claude
    ));
    assert!(!is_tokenfire_owned_command_for_source(
        "sh -c '/tmp/token-fire-hook --source claude --owner token-fire'",
        TokenSourceKind::Claude
    ));
}

#[test]
fn claude_install_writes_tokenfire_hooks_for_required_events() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    fs::write(
        &config,
        r#"{
  "theme": "dark",
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          { "type": "command", "command": "external-tool" }
        ]
      }
    ]
  }
}"#,
    )
    .unwrap();
    let hook = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));

    let result = manager.install(&hook).unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();

    assert!(result.changed);
    assert_eq!(value["theme"], "dark");
    for event in ["Stop", "StopFailure", "SubagentStop"] {
        let groups = value["hooks"][event].as_array().unwrap();
        let commands = groups
            .iter()
            .flat_map(|group| group["hooks"].as_array().unwrap())
            .filter_map(|hook| hook["command"].as_str())
            .collect::<Vec<_>>();
        assert!(commands.iter().any(|command| {
            is_tokenfire_owned_command_for_source(command, TokenSourceKind::Claude)
        }));
    }
}

#[test]
fn claude_uninstall_removes_only_tokenfire_owned_commands() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));
    manager.install(&hook).unwrap();

    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    value["hooks"]["Stop"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "type": "command", "command": "external-tool" }));
    value["hooks"]["Stop"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "type": "command",
            "command": tokenfire_hook_command(&hook, TokenSourceKind::Cursor)
        }));
    fs::write(&config, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    let result = manager.uninstall().unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();

    assert!(result.changed);
    let stop_hooks = value["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
    assert!(stop_hooks
        .iter()
        .any(|hook| hook["command"] == "external-tool"));
    assert!(serde_json::to_string(&value)
        .unwrap()
        .contains("--source cursor"));
    assert!(!serde_json::to_string(&value)
        .unwrap()
        .contains("--source claude"));
}

#[test]
fn claude_install_is_idempotent() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));

    assert!(manager.install(&hook).unwrap().changed);
    let after_first = fs::read_to_string(&config).unwrap();
    assert!(!manager.install(&hook).unwrap().changed);

    let after_second = fs::read_to_string(&config).unwrap();
    assert_eq!(after_second, after_first);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&after_second).unwrap()["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn claude_uninstall_removes_tokenfire_created_empty_groups() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager.install(&hook).unwrap();
    manager.uninstall().unwrap();
    manager.install(&hook).unwrap();
    manager.uninstall().unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();

    for event in ["Stop", "StopFailure", "SubagentStop"] {
        assert_eq!(
            value["hooks"][event].as_array().unwrap().len(),
            0,
            "{event} should not keep TokenFire-created empty groups"
        );
    }
}

#[test]
fn claude_uninstall_preserves_preexisting_empty_groups() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    fs::write(
        &config,
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [] }
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager.install(&hook).unwrap();
    manager.uninstall().unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();

    assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["hooks"]["Stop"][0]["hooks"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn claude_uninstall_preserves_group_metadata_when_tokenfire_command_is_removed() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    fs::write(
        &config,
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "custom",
                        "hooks": [
                            {
                                "type": "command",
                                "command": tokenfire_hook_command(&hook, TokenSourceKind::Claude)
                            }
                        ]
                    }
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager.uninstall().unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();

    assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(value["hooks"]["Stop"][0]["matcher"], "custom");
    assert_eq!(
        value["hooks"]["Stop"][0]["hooks"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn claude_install_preserves_unrelated_hook_events() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    fs::write(
        &config,
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {
                "SessionEnd": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": tokenfire_hook_command(&hook, TokenSourceKind::Claude)
                            }
                        ]
                    }
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager.install(&hook).unwrap();
    manager.uninstall().unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();

    let session_end_command = value["hooks"]["SessionEnd"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(is_tokenfire_owned_command_for_source(
        session_end_command,
        TokenSourceKind::Claude
    ));
}

#[test]
fn claude_malformed_json_does_not_backup_or_write() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let original = r#"{"hooks": "#;
    fs::write(&config, original).unwrap();
    let manager = ClaudeHookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(&dir.path().join("token-fire-hook"))
        .unwrap_err();

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn claude_install_detects_concurrent_config_change() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    fs::write(&config, r#"{"hooks":{}}"#).unwrap();
    let external_update =
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"external edit"}]}]}}"#;
    let manager = ClaudeHookConfigManager::new_with_before_write_hook_for_test(
        config.clone(),
        dir.path().join("backups"),
        {
            let config = config.clone();
            move || {
                fs::write(&config, external_update).unwrap();
            }
        },
    );

    let error = manager
        .install(&dir.path().join("token-fire-hook"))
        .unwrap_err();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(error
        .to_string()
        .contains("settings.json changed during write"));
    assert_eq!(updated, external_update);
    assert!(!updated.contains("token-fire-hook"));
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn claude_status_reports_installed_hook() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    make_executable(&hook);
    let manager = ClaudeHookConfigManager::new(config, dir.path().join("backups"));

    manager.install(&hook).unwrap();
    let status = manager.status().unwrap();

    assert_eq!(status.source, TokenSourceKind::Claude);
    assert!(status.hook_registered);
    assert!(status.hook_executable_exists);
    assert!(status.config_detected);
    assert_eq!(status.config_error, None);
}

#[test]
fn claude_status_reports_uninstalled_config() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    fs::write(
        &config,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"external"}]}]}}"#,
    )
    .unwrap();
    let manager = ClaudeHookConfigManager::new(config, dir.path().join("backups"));

    let status = manager.status().unwrap();

    assert_eq!(status.source, TokenSourceKind::Claude);
    assert!(!status.hook_registered);
    assert!(!status.hook_executable_exists);
    assert!(status.config_detected);
    assert_eq!(status.config_error, None);
}

#[test]
fn claude_status_reports_missing_executable() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("settings.json");
    let hook = dir.path().join("missing").join("token-fire-hook");
    let manager = ClaudeHookConfigManager::new(config, dir.path().join("backups"));

    manager.install(&hook).unwrap();
    let status = manager.status().unwrap();

    assert_eq!(status.source, TokenSourceKind::Claude);
    assert!(status.hook_registered);
    assert!(!status.hook_executable_exists);
    assert!(status.config_detected);
    assert_eq!(status.config_error, None);
}

#[test]
fn cursor_install_writes_versioned_stop_hook() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(
        &config,
        r#"{ "version": 1, "hooks": { "submit": [{ "command": "external" }] } }"#,
    )
    .unwrap();
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    let manager = CursorHookConfigManager::new(config.clone(), dir.path().join("backups"));

    let result = manager.install(&hook).unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();

    assert!(result.changed);
    assert_eq!(value["version"], 1);
    assert_eq!(value["hooks"]["submit"][0]["command"], "external");
    let stop = value["hooks"]["stop"].as_array().unwrap();
    assert!(stop.iter().any(|entry| {
        is_tokenfire_owned_command_for_source(
            entry["command"].as_str().unwrap(),
            TokenSourceKind::Cursor,
        )
    }));
}

#[test]
fn cursor_install_is_idempotent_and_uninstall_preserves_external_entries() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    let manager = CursorHookConfigManager::new(config.clone(), dir.path().join("backups"));

    assert!(manager.install(&hook).unwrap().changed);
    assert!(!manager.install(&hook).unwrap().changed);

    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    value["hooks"]["stop"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "command": "external" }));
    value["hooks"]["stop"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "command": tokenfire_hook_command(&hook, TokenSourceKind::Claude)
        }));
    fs::write(&config, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    assert!(manager.uninstall().unwrap().changed);
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();
    assert_eq!(value["hooks"]["stop"][0]["command"], "external");
    assert!(serde_json::to_string(&value)
        .unwrap()
        .contains("--source claude"));
    assert!(!serde_json::to_string(&value)
        .unwrap()
        .contains("--source cursor"));
}

#[test]
fn cursor_malformed_json_does_not_backup_or_write() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    let original = r#"{"hooks": "#;
    fs::write(&config, original).unwrap();
    let manager = CursorHookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(&dir.path().join("token-fire-hook"))
        .unwrap_err();

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn cursor_install_detects_concurrent_config_change() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(&config, r#"{"version":1,"hooks":{}}"#).unwrap();
    let external_update = r#"{"version":1,"hooks":{"stop":[{"command":"external edit"}]}}"#;
    let manager = CursorHookConfigManager::new_with_before_write_hook_for_test(
        config.clone(),
        dir.path().join("backups"),
        {
            let config = config.clone();
            move || {
                fs::write(&config, external_update).unwrap();
            }
        },
    );

    let error = manager
        .install(&dir.path().join("token-fire-hook"))
        .unwrap_err();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(error
        .to_string()
        .contains("hooks.json changed during write"));
    assert_eq!(updated, external_update);
    assert!(!updated.contains("token-fire-hook"));
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn cursor_status_reports_installed_hook() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    let hook = dir.path().join("token-fire-hook");
    fs::write(&hook, b"#!/bin/sh\n").unwrap();
    make_executable(&hook);
    let manager = CursorHookConfigManager::new(config, dir.path().join("backups"));

    manager.install(&hook).unwrap();
    let status = manager.status().unwrap();

    assert_eq!(status.source, TokenSourceKind::Cursor);
    assert!(status.hook_registered);
    assert!(status.hook_executable_exists);
    assert!(status.config_detected);
    assert_eq!(status.config_error, None);
}

#[test]
fn cursor_status_reports_uninstalled_config() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(
        &config,
        r#"{"version":1,"hooks":{"stop":[{"command":"external"}]}}"#,
    )
    .unwrap();
    let manager = CursorHookConfigManager::new(config, dir.path().join("backups"));

    let status = manager.status().unwrap();

    assert_eq!(status.source, TokenSourceKind::Cursor);
    assert!(!status.hook_registered);
    assert!(!status.hook_executable_exists);
    assert!(status.config_detected);
    assert_eq!(status.config_error, None);
}

#[test]
fn cursor_status_reports_missing_executable() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    let hook = dir.path().join("missing").join("token-fire-hook");
    let manager = CursorHookConfigManager::new(config, dir.path().join("backups"));

    manager.install(&hook).unwrap();
    let status = manager.status().unwrap();

    assert_eq!(status.source, TokenSourceKind::Cursor);
    assert!(status.hook_registered);
    assert!(!status.hook_executable_exists);
    assert!(status.config_detected);
    assert_eq!(status.config_error, None);
}

#[test]
fn install_preserves_flux_unknown_fields_and_appends_stop_hook() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(&config, include_str!("fixtures/traecli.toml")).unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(updated.contains("flux-hook"));
    assert!(updated.contains("[unknown]"));
    assert!(updated.contains("token-fire-hook"));
    assert!(updated.contains("--owner token-fire"));
    let doc = updated.parse::<DocumentMut>().unwrap();
    assert!(doc.get("Stop").is_some());
    assert!(doc.get("Notification").is_some());
    assert!(doc.get("hooks").is_none());
    assert_eq!(fs::read_dir(dir.path().join("backups")).unwrap().count(), 1);
}

#[test]
fn install_appends_stop_hook_when_stop_is_array_of_tables() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(
        &config,
        r#"[[Stop.hooks]]
type = "command"
command = "'/Applications/Flux Island.app/Contents/MacOS/Flux Island' --source traex"
"#,
    )
    .unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(updated.contains("Flux Island"));
    assert!(updated.contains("token-fire-hook"));
    assert!(updated.contains("--owner token-fire"));
    let doc = updated.parse::<DocumentMut>().unwrap();
    assert!(doc.get("Stop").is_some());
    assert!(doc.get("hooks").is_none());
}

#[test]
fn repeated_install_is_idempotent_when_stop_is_array_of_tables() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(
        &config,
        r#"[[Stop.hooks]]
type = "command"
command = "'/Applications/Flux Island.app/Contents/MacOS/Flux Island' --source traex"
"#,
    )
    .unwrap();
    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    let first = manager.install(&hook_path).unwrap();
    let after_first = fs::read_to_string(&config).unwrap();
    let second = manager.install(&hook_path).unwrap();
    let updated = fs::read_to_string(&config).unwrap();

    assert!(first.changed);
    assert!(!second.changed);
    assert_eq!(updated, after_first);
    assert_eq!(updated.matches("--owner token-fire").count(), 1);
}

#[test]
fn install_migrates_legacy_tokenfire_hook_to_top_level_stop_hooks() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(
        &config,
        r#"[hooks]
[[hooks.Stop.hooks]]
command = "'/Applications/Old.app/Contents/MacOS/token-fire-hook' --source traex --owner token-fire"
type = "command"
timeout = 5

[unknown]
keep = true
"#,
    )
    .unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    let doc = updated.parse::<DocumentMut>().unwrap();
    assert!(doc.get("Stop").is_some());
    assert!(doc.get("hooks").is_some());
    assert!(updated.contains("[unknown]"));
    assert!(!updated.contains("Old.app"));
    assert!(updated.contains("TokenFire.app"));
    assert_eq!(updated.matches("--owner token-fire").count(), 1);
}

#[test]
fn install_preserves_legacy_non_tokenfire_hooks_while_migrating_tokenfire() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(
        &config,
        r#"[hooks.state."/Users/example/.trae/traecli.toml:stop:0:0"]
trusted_hash = "sha256:abc123"

[[hooks.SessionStart]]
matcher = "startup|resume|clear|compact"

  [[hooks.SessionStart.hooks]]
  type = "command"
  command = "echo legacy"

[[hooks.Stop]]
[[hooks.Stop.hooks]]
command = "'/Applications/Old.app/Contents/MacOS/token-fire-hook' --source traex --owner token-fire"
type = "command"
timeout = 5

[[hooks.Stop.hooks]]
type = "command"
command = "[ -e '/Applications/Flux Island.app/Contents/MacOS/Flux Island' ] || exit 0; flux-hooks --source traex"
"#,
    )
    .unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    let doc = updated.parse::<DocumentMut>().unwrap();
    assert!(doc.get("Stop").is_some());
    assert!(doc.get("hooks").is_some());
    assert!(updated.contains("[hooks.state"));
    assert!(updated.contains("echo legacy"));
    assert!(updated.contains("Flux Island"));
    assert!(!updated.contains("Old.app"));
    assert!(updated.contains("TokenFire.app"));
    assert_eq!(updated.matches("--owner token-fire").count(), 1);
}

#[test]
fn install_creates_config_when_file_is_missing() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("missing").join("traecli.toml");
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(updated.contains("token-fire-hook"));
    assert!(updated.contains("--owner token-fire"));
}

#[test]
fn install_returns_read_errors_other_than_missing_file() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::create_dir(&config).unwrap();
    let manager = HookConfigManager::new(config, dir.path().join("backups"));

    let error = manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap_err();

    assert!(error.to_string().contains("traecli.toml"));
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn install_returns_malformed_config_error_without_backup_or_write() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    let original = "[hooks\nnot valid toml";
    fs::write(&config, original).unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap_err();

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn install_rejects_non_table_hooks_without_backup_or_write() {
    assert_install_rejects_unexpected_shape_without_side_effects(
        r#"hooks = "custom"
"#,
        "hooks",
    );
}

#[test]
fn install_rejects_non_array_stop_hooks_without_backup_or_write() {
    assert_install_rejects_unexpected_shape_without_side_effects(
        r#"[Stop]
hooks = "custom"
"#,
        "Stop.hooks",
    );
}

#[test]
fn install_preserves_legacy_non_tokenfire_stop_hooks() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(
        &config,
        r#"[hooks]
[[hooks.Stop.hooks]]
command = "'/Applications/Other.app/hook' --owner other"
type = "command"
timeout = 5
"#,
    )
    .unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(updated.contains("--owner other"));
    assert!(updated.contains("--owner token-fire"));
}

#[test]
fn install_preserves_hooks_state_and_appends_top_level_stop_hook() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(
        &config,
        r#"[hooks.state."/Users/example/.trae/hooks.json:stop:0:0"]
trusted_hash = "sha256:abc123"
"#,
    )
    .unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    let doc = updated.parse::<DocumentMut>().unwrap();
    assert!(doc.get("Stop").is_some());
    assert!(updated.contains("[hooks.state"));
    assert!(updated.contains("trusted_hash"));
    assert!(updated.contains("token-fire-hook"));
    assert_eq!(updated.matches("--owner token-fire").count(), 1);
}

#[test]
fn repeated_install_is_idempotent_and_moved_app_replaces_old_hook() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(&config, include_str!("fixtures/traecli.toml")).unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(&dir.path().join("Old.app/Contents/MacOS/token-fire-hook"))
        .unwrap();
    manager
        .install(&dir.path().join("New.app/Contents/MacOS/token-fire-hook"))
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(!updated.contains("Old.app"));
    assert_eq!(updated.matches("--owner token-fire").count(), 1);
    assert!(updated.contains("New.app"));
}

#[test]
fn identical_repeated_install_does_not_backup_or_rewrite_config() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(&config, include_str!("fixtures/traecli.toml")).unwrap();
    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    let first = manager.install(&hook_path).unwrap();
    let after_first = fs::read_to_string(&config).unwrap();
    let second = manager.install(&hook_path).unwrap();

    assert!(first.changed);
    assert!(!second.changed);
    assert_eq!(fs::read_to_string(&config).unwrap(), after_first);
    assert_eq!(fs::read_dir(dir.path().join("backups")).unwrap().count(), 1);
}

#[test]
fn rapid_operations_create_distinct_backup_files() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(&config, include_str!("fixtures/traecli.toml")).unwrap();
    let manager = HookConfigManager::new(config, dir.path().join("backups"));

    for index in 0..4 {
        manager
            .install(&dir.path().join(format!(
                "TokenFire{index}.app/Contents/MacOS/token-fire-hook"
            )))
            .unwrap();
    }

    assert_eq!(fs::read_dir(dir.path().join("backups")).unwrap().count(), 4);
}

#[test]
fn uninstall_removes_only_tokenfire_owned_hooks() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(
        &config,
        r#"[hooks]
[[hooks.Stop.hooks]]
command = "'/Applications/TokenFire.app/Contents/MacOS/token-fire-hook' --source traex --owner token-fire"
type = "command"
timeout = 5
[[hooks.Stop.hooks]]
command = "'/Applications/Other.app/hook' --owner other"
type = "command"
timeout = 5
"#,
    )
    .unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager.uninstall().unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(!updated.contains("--owner token-fire"));
    assert!(updated.contains("--owner other"));
}

#[test]
fn no_op_uninstall_does_not_backup_or_rewrite_config() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    let original = include_str!("fixtures/traecli.toml");
    fs::write(&config, original).unwrap();
    let before = fs::metadata(&config).unwrap().modified().unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    let result = manager.uninstall().unwrap();

    assert!(!result.changed);
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), before);
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn tokenfire_hook_predicate_requires_binary_owner_and_source() {
    assert!(is_tokenfire_hook(
        "'/x/token-fire-hook' --source traex --owner token-fire"
    ));
    assert!(!is_tokenfire_hook(
        "'/x/token-fire-hook' --owner token-fire"
    ));
    assert!(!is_tokenfire_hook(
        "'/x/token-fire-hook' --source codex --owner token-fire"
    ));
    assert!(!is_tokenfire_hook("'/x/token-fire-hook' --source traex"));
    assert!(!is_tokenfire_hook("'other-hook' --owner token-fire"));
}

#[test]
fn install_escapes_single_quotes_in_hook_path() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(&config, include_str!("fixtures/traecli.toml")).unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("Token'Fire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    let doc = updated.parse::<DocumentMut>().unwrap();
    let command = doc["Stop"]["hooks"]
        .as_array_of_tables()
        .unwrap()
        .iter()
        .filter_map(|table| table.get("command").and_then(toml_edit::Item::as_str))
        .find(|command| command.contains("--owner token-fire"))
        .unwrap();
    assert!(command.contains("Token'\"'\"'Fire.app/Contents/MacOS/token-fire-hook'"));
}

#[test]
fn codex_install_preserves_existing_hooks_and_appends_stop_hook() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(&config, include_str!("fixtures/codex-hooks.json")).unwrap();
    let manager = CodexHookConfigManager::new(config.clone(), dir.path().join("backups"));

    let result = manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap();

    assert!(result.changed);
    let updated = fs::read_to_string(&config).unwrap();
    let json: serde_json::Value = serde_json::from_str(&updated).unwrap();
    assert!(updated.contains("Flux Island"));
    assert!(updated.contains("PermissionRequest"));
    assert!(updated.contains(r#""matcher":"*""#) || updated.contains(r#""matcher": "*""#));
    assert!(updated.contains("token-fire-hook"));
    assert!(updated.contains("--source codex"));
    assert!(updated.contains("--owner token-fire"));
    assert_eq!(fs::read_dir(dir.path().join("backups")).unwrap().count(), 1);
    assert!(json.pointer("/hooks/Stop").is_some());
}

#[test]
fn codex_install_is_idempotent() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(&config, include_str!("fixtures/codex-hooks.json")).unwrap();
    let hook_path = dir
        .path()
        .join("TokenFire.app/Contents/MacOS/token-fire-hook");
    let manager = CodexHookConfigManager::new(config.clone(), dir.path().join("backups"));

    let first = manager.install(&hook_path).unwrap();
    let after_first = fs::read_to_string(&config).unwrap();
    let second = manager.install(&hook_path).unwrap();

    assert!(first.changed);
    assert!(!second.changed);
    assert_eq!(fs::read_to_string(&config).unwrap(), after_first);
    assert_eq!(after_first.matches("--owner token-fire").count(), 1);
}

#[test]
fn codex_uninstall_removes_only_tokenfire_owned_command() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(
        &config,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"'/x/token-fire-hook' --source codex --owner token-fire","timeout":5},{"type":"command","command":"flux hook token-fire text","timeout":5}]}]}}"#,
    )
    .unwrap();
    let manager = CodexHookConfigManager::new(config.clone(), dir.path().join("backups"));

    let result = manager.uninstall().unwrap();

    assert!(result.changed);
    let updated = fs::read_to_string(&config).unwrap();
    assert!(!updated.contains("--owner token-fire"));
    assert!(updated.contains("flux hook token-fire text"));
}

#[test]
fn codex_malformed_json_does_not_backup_or_write() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    let original = r#"{"hooks": "#;
    fs::write(&config, original).unwrap();
    let before = fs::metadata(&config).unwrap().modified().unwrap();
    let manager = CodexHookConfigManager::new(config.clone(), dir.path().join("backups"));

    manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap_err();

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), before);
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn codex_install_detects_concurrent_config_change() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(&config, include_str!("fixtures/codex-hooks.json")).unwrap();
    let external_update = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"external edit","timeout":9}]}]}}"#;
    let manager = CodexHookConfigManager::new_with_before_write_hook_for_test(
        config.clone(),
        dir.path().join("backups"),
        {
            let config = config.clone();
            move || {
                fs::write(&config, external_update).unwrap();
            }
        },
    );

    let error = manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap_err();

    let updated = fs::read_to_string(&config).unwrap();
    assert!(error
        .to_string()
        .contains("hooks.json changed during write"));
    assert_eq!(updated, external_update);
    assert!(updated.contains("external edit"));
    assert!(!updated.contains("token-fire-hook"));
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn codex_parallel_installs_from_two_managers_do_not_conflict_or_duplicate_tmp() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("hooks.json");
    fs::write(&config, include_str!("fixtures/codex-hooks.json")).unwrap();
    let backups = dir.path().join("backups");
    let first = Arc::new(CodexHookConfigManager::new(config.clone(), backups.clone()));
    let second = Arc::new(CodexHookConfigManager::new(config.clone(), backups));
    let first_hook = dir.path().join("First.app/Contents/MacOS/token-fire-hook");
    let second_hook = dir.path().join("Second.app/Contents/MacOS/token-fire-hook");

    let first_thread = {
        let manager = first.clone();
        let hook = first_hook.clone();
        thread::spawn(move || manager.install(&hook))
    };
    let second_thread = {
        let manager = second.clone();
        let hook = second_hook.clone();
        thread::spawn(move || manager.install(&hook))
    };

    first_thread.join().unwrap().unwrap();
    second_thread.join().unwrap().unwrap();

    let updated = fs::read_to_string(&config).unwrap();
    assert_eq!(updated.matches("--owner token-fire").count(), 1);
    assert!(!dir.path().join("hooks.json.tmp").exists());
    assert!(!dir.path().join("hooks.json.lock").exists());
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn assert_install_rejects_unexpected_shape_without_side_effects(
    original: &str,
    expected_error: &str,
) {
    let dir = tempdir().unwrap();
    let config = dir.path().join("traecli.toml");
    fs::write(&config, original).unwrap();
    let manager = HookConfigManager::new(config.clone(), dir.path().join("backups"));

    let error = manager
        .install(
            &dir.path()
                .join("TokenFire.app/Contents/MacOS/token-fire-hook"),
        )
        .unwrap_err();

    assert!(error.to_string().contains(expected_error));
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!dir.path().join("backups").exists());
}
