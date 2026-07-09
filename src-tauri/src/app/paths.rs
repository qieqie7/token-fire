use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub home: PathBuf,
    pub database: PathBuf,
    pub run_dir: PathBuf,
    pub socket: PathBuf,
    pub logs_dir: PathBuf,
    pub app_log: PathBuf,
    pub hook_log: PathBuf,
    pub parser_log: PathBuf,
    pub db_log: PathBuf,
    pub backups_dir: PathBuf,
    pub debug_bundles_dir: PathBuf,
}

pub fn tokenfire_home() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory not found"))?;
    Ok(home.join(".token-fire"))
}

pub fn runtime_paths() -> anyhow::Result<RuntimePaths> {
    let home = tokenfire_home()?;
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
