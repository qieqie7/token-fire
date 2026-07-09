use token_fire::app::paths::runtime_paths;

#[test]
fn runtime_paths_are_under_token_fire_home() {
    let paths = runtime_paths().expect("runtime paths");

    assert!(paths.home.ends_with(".token-fire"));
    assert_eq!(paths.database, paths.home.join("token-fire.sqlite"));
    assert_eq!(paths.socket, paths.home.join("run").join("token-fire.sock"));
    assert_eq!(paths.app_log, paths.home.join("logs").join("app.log"));
    assert_eq!(paths.hook_log, paths.home.join("logs").join("hook.log"));
    assert_eq!(paths.parser_log, paths.home.join("logs").join("parser.log"));
    assert_eq!(paths.db_log, paths.home.join("logs").join("db.log"));
    assert_eq!(paths.backups_dir, paths.home.join("backups"));
    assert_eq!(paths.debug_bundles_dir, paths.home.join("debug-bundles"));
}
