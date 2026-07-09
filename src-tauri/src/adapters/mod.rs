use serde::{Deserialize, Serialize};

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod hook_command;
pub mod source;
pub mod traex;
pub mod transcript;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookMetadata {
    pub source: Option<String>,
    pub hook_event_name: Option<String>,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub conversation_id: Option<String>,
    pub turn_id: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub timestamp: Option<String>,
}
