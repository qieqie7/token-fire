use std::io::Read;
use std::path::PathBuf;

use serde_json::json;
use token_fire::adapters::traex::hook_payload::filter_hook_payload_with_source;
use token_fire::app::build_identity::{
    current_build_identity, fields_with_build_identity, has_version_json_arg, print_version_json,
};
use token_fire::app::logging::append_hook_log;
use token_fire::app::paths::{runtime_paths, RuntimePaths};
use token_fire::app::socket_server::forward_hook_metadata;

fn main() {
    let identity = current_build_identity();
    if has_version_json_arg() {
        let _ = print_version_json(&identity);
        std::process::exit(0);
    }

    if let Err(error) = run(identity) {
        let paths = paths_from_env().or_else(|_| runtime_paths());
        if let Ok(paths) = paths {
            let identity = current_build_identity();
            let _ = append_hook_log(
                &paths,
                "warn",
                "hook_internal_failure",
                fields_with_build_identity(json!({ "error_kind": error.to_string() }), &identity),
            );
        }
    }
    std::process::exit(0);
}

fn run(identity: token_fire::app::build_identity::BuildIdentity) -> anyhow::Result<()> {
    let paths = paths_from_env().or_else(|_| runtime_paths())?;
    let mut stdin = String::new();
    std::io::stdin().read_to_string(&mut stdin)?;
    let payload: serde_json::Value = match serde_json::from_str(&stdin) {
        Ok(value) => value,
        Err(error) => {
            append_hook_log(
                &paths,
                "warn",
                "hook_malformed_payload",
                fields_with_build_identity(json!({ "error_kind": error.to_string() }), &identity),
            )?;
            return Ok(());
        }
    };
    let cli_source = cli_source_arg();
    let metadata =
        filter_hook_payload_with_source(payload, cli_source.as_deref().or(Some("traex")));
    let socket_path = std::env::var_os("TOKEN_FIRE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.socket.clone());
    match forward_hook_metadata(&socket_path, &metadata) {
        Ok(()) => append_hook_log(
            &paths,
            "info",
            "hook_forwarded",
            fields_with_build_identity(
                json!({
                    "source": metadata.source,
                    "hook_path": current_hook_path()
                        .map(|path| path.to_string_lossy().to_string())
                        .ok()
                }),
                &identity,
            ),
        )?,
        Err(error) => append_hook_log(
            &paths,
            "warn",
            "hook_socket_unavailable",
            fields_with_build_identity(json!({ "error_kind": error.to_string() }), &identity),
        )?,
    }
    Ok(())
}

fn cli_source_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--source" {
            return args
                .next()
                .filter(|value| matches!(value.as_str(), "traex" | "codex" | "claude" | "cursor"));
        }
    }
    None
}

fn current_hook_path() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_exe()?.canonicalize()?)
}

fn paths_from_env() -> anyhow::Result<RuntimePaths> {
    let home = std::env::var_os("TOKEN_FIRE_HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("TOKEN_FIRE_HOME is not set"))?;
    let run_dir = home.join("run");
    let logs_dir = home.join("logs");
    Ok(RuntimePaths {
        database: home.join("token-fire.sqlite"),
        socket: run_dir.join("token-fire.sock"),
        app_log: logs_dir.join("app.log"),
        hook_log: logs_dir.join("hook.log"),
        parser_log: logs_dir.join("parser.log"),
        db_log: logs_dir.join("db.log"),
        backups_dir: home.join("backups"),
        debug_bundles_dir: home.join("debug-bundles"),
        home,
        run_dir,
        logs_dir,
    })
}
