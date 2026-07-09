#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuLabels {
    pub source_submenu: &'static str,
    pub traex_source: &'static str,
    pub codex_source: &'static str,
    pub claude_source: &'static str,
    pub cursor_source: &'static str,
    pub pause_tracking: &'static str,
    pub resume_tracking: &'static str,
    pub open_logs: &'static str,
    pub copy_debug_bundle: &'static str,
    pub enable_debug_logging: &'static str,
    pub quit: &'static str,
}

pub fn menu_labels() -> MenuLabels {
    MenuLabels {
        source_submenu: "来源",
        traex_source: "TraeX",
        codex_source: "Codex",
        claude_source: "Claude",
        cursor_source: "Cursor",
        pause_tracking: "暂停统计",
        resume_tracking: "继续统计",
        open_logs: "打开日志目录",
        copy_debug_bundle: "复制诊断包",
        enable_debug_logging: "开启调试日志",
        quit: "退出",
    }
}

pub use crate::app::state::MenuAction;
