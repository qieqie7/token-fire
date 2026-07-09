use std::io::Read;
use std::os::unix::net::UnixListener;
use std::thread;

use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn hook_filters_allowed_metadata_and_forwards_to_socket() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.env("TOKEN_FIRE_SOCKET", &socket_path)
        .write_stdin(
            r#"{"source":"traex","hook_event_name":"Stop","session_id":"019","transcript_path":"/tmp/rollout-019.jsonl","turn_id":"turn","model":"m","cwd":"/tmp/project","timestamp":"2026-06-20T03:00:00Z","prompt":"secret"}"#,
        )
        .assert()
        .success();

    let forwarded = handle.join().unwrap();
    assert!(forwarded.contains(r#""session_id":"019""#));
    assert!(!forwarded.contains("secret"));
    assert!(!forwarded.contains("prompt"));
}

#[test]
fn hook_forwarded_log_records_current_executable_path() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.env("TOKEN_FIRE_SOCKET", &socket_path)
        .env("TOKEN_FIRE_HOME", dir.path())
        .write_stdin(r#"{"source":"traex","hook_event_name":"Stop"}"#)
        .assert()
        .success();
    handle.join().unwrap();

    let hook_log = std::fs::read_to_string(dir.path().join("logs").join("hook.log")).unwrap();
    assert!(hook_log.contains("hook_forwarded"));
    assert!(hook_log.contains("\"hook_path\""));
    assert!(hook_log.contains("token-fire-hook"));
}

#[test]
fn hook_exits_zero_and_logs_warn_when_socket_is_unavailable() {
    let dir = tempdir().unwrap();
    let missing_socket = dir.path().join("missing.sock");

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.env("TOKEN_FIRE_SOCKET", &missing_socket)
        .env("TOKEN_FIRE_HOME", dir.path())
        .write_stdin(r#"{"source":"traex","hook_event_name":"Stop"}"#)
        .assert()
        .success();

    let hook_log = std::fs::read_to_string(dir.path().join("logs").join("hook.log")).unwrap();
    assert!(hook_log.contains("hook_socket_unavailable"));
    assert!(hook_log.contains(r#""level":"warn""#));
}

#[test]
fn hook_exits_zero_and_logs_warn_for_malformed_payload() {
    let dir = tempdir().unwrap();

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.env("TOKEN_FIRE_HOME", dir.path())
        .write_stdin("{not-json")
        .assert()
        .success();

    let hook_log = std::fs::read_to_string(dir.path().join("logs").join("hook.log")).unwrap();
    assert!(hook_log.contains("hook_malformed_payload"));
    assert!(hook_log.contains(r#""level":"warn""#));
}

#[test]
fn hook_cli_source_argument_fills_missing_payload_source() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.arg("--source")
        .arg("codex")
        .arg("--owner")
        .arg("token-fire")
        .env("TOKEN_FIRE_SOCKET", &socket_path)
        .write_stdin(
            r#"{"hook_event_name":"Stop","session_id":"019","transcript_path":"/tmp/rollout-019.jsonl","prompt":"secret","tool_payload":{"secret":true}}"#,
        )
        .assert()
        .success();

    let forwarded = handle.join().unwrap();
    assert!(forwarded.contains(r#""source":"codex""#));
    assert!(forwarded.contains(r#""session_id":"019""#));
    assert!(!forwarded.contains("secret"));
    assert!(!forwarded.contains("tool_payload"));
    assert!(!forwarded.contains("prompt"));
}

#[test]
fn hook_cli_source_argument_overrides_payload_source() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.arg("--source")
        .arg("codex")
        .arg("--owner")
        .arg("token-fire")
        .env("TOKEN_FIRE_SOCKET", &socket_path)
        .write_stdin(
            r#"{"source":"traex","hook_event_name":"Stop","session_id":"019","transcript_path":"/tmp/rollout-019.jsonl"}"#,
        )
        .assert()
        .success();

    let forwarded = handle.join().unwrap();
    assert!(forwarded.contains(r#""source":"codex""#));
    assert!(!forwarded.contains(r#""source":"traex""#));
}

#[test]
fn hook_without_source_argument_uses_traex_app_owned_default() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.env("TOKEN_FIRE_SOCKET", &socket_path)
        .write_stdin(r#"{"source":"codex","hook_event_name":"Stop","session_id":"019"}"#)
        .assert()
        .success();

    let forwarded = handle.join().unwrap();
    assert!(forwarded.contains(r#""source":"traex""#));
    assert!(!forwarded.contains(r#""source":"codex""#));
}

#[test]
fn hook_cli_rejects_unknown_source_to_traex_fallback() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.arg("--source")
        .arg("unknown")
        .env("TOKEN_FIRE_SOCKET", &socket_path)
        .write_stdin(r#"{"hook_event_name":"Stop"}"#)
        .assert()
        .success();

    let forwarded = handle.join().unwrap();
    assert!(forwarded.contains(r#""source":"traex""#));
}

#[test]
fn hook_cli_accepts_cursor_source_and_conversation_id_without_content() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.arg("--source")
        .arg("cursor")
        .env("TOKEN_FIRE_SOCKET", &socket_path)
        .write_stdin(
            r#"{"hook_event_name":"stop","conversation_id":"cursor-conv-1","prompt":"SENTINEL_SECRET_PROMPT","messages":[{"text":"SENTINEL_ASSISTANT_TEXT"}],"tool_payload":{"secret":true},"_sshClient":"remote","_hostname":"host","terminal_app":"Terminal"}"#,
        )
        .assert()
        .success();

    let forwarded = handle.join().unwrap();
    assert!(forwarded.contains(r#""source":"cursor""#));
    assert!(forwarded.contains(r#""conversation_id":"cursor-conv-1""#));
    assert!(!forwarded.contains("SENTINEL_SECRET_PROMPT"));
    assert!(!forwarded.contains("SENTINEL_ASSISTANT_TEXT"));
    assert!(!forwarded.contains("tool_payload"));
    assert!(!forwarded.contains("_sshClient"));
    assert!(!forwarded.contains("_hostname"));
    assert!(!forwarded.contains("terminal_app"));
}

#[test]
fn hook_cli_accepts_claude_source_without_content() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("token-fire.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    });

    let mut cmd = Command::cargo_bin("token-fire-hook").unwrap();
    cmd.arg("--source")
        .arg("claude")
        .env("TOKEN_FIRE_SOCKET", &socket_path)
        .write_stdin(
            r#"{"hook_event_name":"Stop","transcript_path":"/tmp/claude.jsonl","prompt":"SENTINEL_SECRET_PROMPT","tool_input":"SENTINEL_TOOL_INPUT"}"#,
        )
        .assert()
        .success();

    let forwarded = handle.join().unwrap();
    assert!(forwarded.contains(r#""source":"claude""#));
    assert!(forwarded.contains(r#""transcript_path":"/tmp/claude.jsonl""#));
    assert!(!forwarded.contains("SENTINEL_SECRET_PROMPT"));
    assert!(!forwarded.contains("SENTINEL_TOOL_INPUT"));
    assert!(!forwarded.contains("prompt"));
}
