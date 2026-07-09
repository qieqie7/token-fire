use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    emit_build_identity();
    tauri_build::build();
}

fn emit_build_identity() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    if let Some(head_path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }
    println!("cargo:rerun-if-env-changed=TOKEN_FIRE_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=TOKEN_FIRE_GIT_COMMIT_SHORT");
    println!("cargo:rerun-if-env-changed=TOKEN_FIRE_GIT_DIRTY");
    println!("cargo:rerun-if-env-changed=TOKEN_FIRE_BUILD_TIME");

    let git_commit = std::env::var("TOKEN_FIRE_GIT_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let git_commit_short = std::env::var("TOKEN_FIRE_GIT_COMMIT_SHORT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output(&["rev-parse", "--short=7", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = std::env::var("TOKEN_FIRE_GIT_DIRTY")
        .ok()
        .filter(|value| value == "true" || value == "false")
        .map(|value| value == "true")
        .unwrap_or_else(|| {
            git_output(&["status", "--porcelain", "--untracked-files=normal"])
                .map(|status| !status.trim().is_empty())
                .unwrap_or(true)
        });
    let build_time = std::env::var("TOKEN_FIRE_BUILD_TIME").unwrap_or_else(|_| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("unix:{seconds}")
    });

    println!("cargo:rustc-env=TOKEN_FIRE_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=TOKEN_FIRE_GIT_COMMIT_SHORT={git_commit_short}");
    println!("cargo:rustc-env=TOKEN_FIRE_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=TOKEN_FIRE_BUILD_TIME={build_time}");
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
