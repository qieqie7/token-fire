use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::adapters::source::TokenSourceKind;
use crate::adapters::HookMetadata;
use crate::core::observation::{NormalizedObservation, SourceRecordIdConfidence};

pub mod hook_config;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorWatermark {
    version: u8,
    #[serde(default)]
    transcript_path_hash: String,
    #[serde(default, skip_serializing)]
    transcript_path: Option<String>,
    #[serde(default)]
    committed_prefix_hash: String,
    len: u64,
    modified_unix_ms: i64,
    committed_offset: u64,
}

impl CursorWatermark {
    fn matches_transcript_path_hash(&self, transcript_path_hash: &str) -> bool {
        self.transcript_path_hash == transcript_path_hash
            || self
                .transcript_path
                .as_deref()
                .is_some_and(|path| stable_digest(path.as_bytes()) == transcript_path_hash)
    }

    fn matches_file_content(&self, content: &str, file_len: u64, modified_unix_ms: i64) -> bool {
        if self.committed_offset > file_len || self.len > file_len {
            return false;
        }

        let committed_offset = self.committed_offset as usize;
        if committed_offset > content.len() || !content.is_char_boundary(committed_offset) {
            return false;
        }

        if self.committed_prefix_hash.is_empty() {
            return self.len == file_len && self.modified_unix_ms == modified_unix_ms;
        }

        stable_digest(&content.as_bytes()[..committed_offset]) == self.committed_prefix_hash
    }

    fn without_raw_path(&self, transcript_path_hash: &str) -> Self {
        let mut sanitized = self.clone();
        sanitized.transcript_path_hash = transcript_path_hash.to_string();
        sanitized.transcript_path = None;
        sanitized
    }

    fn for_current_transcript(
        &self,
        transcript_path_hash: &str,
        content: &str,
        file_len: u64,
        modified_unix_ms: i64,
    ) -> anyhow::Result<Self> {
        let committed_offset = self.committed_offset as usize;
        if committed_offset > content.len() || !content.is_char_boundary(committed_offset) {
            anyhow::bail!("invalid cursor watermark offset");
        }

        Ok(Self {
            version: self.version,
            transcript_path_hash: transcript_path_hash.to_string(),
            transcript_path: None,
            committed_prefix_hash: stable_digest(&content.as_bytes()[..committed_offset]),
            len: file_len,
            modified_unix_ms,
            committed_offset: self.committed_offset,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
struct CursorRow {
    role: CursorRole,
    offset: u64,
    end_offset: u64,
    tokens: i64,
}

#[derive(Debug, Clone, Copy)]
struct TokenSelection {
    input_tokens: i64,
    output_tokens: i64,
    start_offset: u64,
    end_offset: u64,
}

#[derive(Debug, Clone)]
pub struct PendingCursorCollection {
    observation: NormalizedObservation,
    watermark_path: PathBuf,
    watermark: CursorWatermark,
    legacy_watermark_path: Option<PathBuf>,
}

impl PendingCursorCollection {
    pub fn observation(&self) -> &NormalizedObservation {
        &self.observation
    }

    pub fn commit(self) -> anyhow::Result<()> {
        write_watermark(&self.watermark_path, &self.watermark)?;
        if let Some(legacy_path) = self.legacy_watermark_path {
            remove_watermark_if_exists(&legacy_path)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorEmptyReason {
    TranscriptPathMissing,
    TranscriptUnreadable,
    NoCompleteJsonlRows,
    NoNewCompleteRound,
    WatermarkAtEof,
}

#[derive(Debug, Clone)]
pub enum CursorCollectResult {
    Pending(PendingCursorCollection),
    Empty(CursorEmptyReason),
}

#[derive(Debug, Clone)]
pub struct CursorTranscriptIdentity {
    record_key: String,
    source_path: String,
    session_id: Option<String>,
    turn_id: Option<String>,
    turn_boundary_id: Option<String>,
}

impl CursorTranscriptIdentity {
    pub fn from_path(transcript_path: &Path, metadata: &HookMetadata) -> Self {
        let path_hash = stable_digest(transcript_path.to_string_lossy().as_bytes());
        let fallback_id = metadata
            .conversation_id
            .clone()
            .or_else(|| metadata.session_id.clone());
        let session_id = metadata.session_id.clone().or_else(|| fallback_id.clone());
        let turn_id = metadata.turn_id.clone().or_else(|| fallback_id.clone());
        let turn_boundary_id = metadata
            .turn_id
            .clone()
            .or_else(|| metadata.conversation_id.clone())
            .or_else(|| metadata.session_id.clone());

        Self {
            record_key: format!("cursor:{path_hash}"),
            source_path: format!("cursor:{path_hash}"),
            session_id,
            turn_id,
            turn_boundary_id,
        }
    }
}

pub fn resolve_cursor_transcript(conversation_id: &str, home: Option<&Path>) -> Option<PathBuf> {
    if validate_conversation_id(conversation_id).is_err() {
        return None;
    }

    let base = match home {
        Some(home) => home.join(".cursor/projects"),
        None => dirs::home_dir()?.join(".cursor/projects"),
    };

    for entry in fs::read_dir(base).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }

        let candidate = entry
            .path()
            .join("agent-transcripts")
            .join(conversation_id)
            .join(format!("{conversation_id}.jsonl"));
        let Ok(candidate_metadata) = candidate.metadata() else {
            continue;
        };
        if candidate_metadata.is_file() && File::open(&candidate).is_ok() {
            return Some(candidate);
        }
    }

    None
}

pub fn collect_for_conversation(
    conversation_id: &str,
    metadata: &HookMetadata,
    home: Option<&Path>,
    watermark_dir: Option<&Path>,
) -> anyhow::Result<Option<NormalizedObservation>> {
    match collect_pending_for_conversation(conversation_id, metadata, home, watermark_dir)? {
        CursorCollectResult::Pending(pending) => {
            let observation = pending.observation().clone();
            pending.commit()?;
            Ok(Some(observation))
        }
        CursorCollectResult::Empty(_) => Ok(None),
    }
}

pub(crate) fn collect_pending_for_conversation(
    conversation_id: &str,
    metadata: &HookMetadata,
    home: Option<&Path>,
    watermark_dir: Option<&Path>,
) -> anyhow::Result<CursorCollectResult> {
    validate_conversation_id(conversation_id)?;
    let Some(transcript_path) = resolve_cursor_transcript(conversation_id, home) else {
        return Ok(CursorCollectResult::Empty(
            CursorEmptyReason::TranscriptPathMissing,
        ));
    };
    let identity = CursorTranscriptIdentity::from_path(&transcript_path, metadata);
    collect_pending_from_transcript_path(&transcript_path, metadata, identity, watermark_dir)
}

pub fn collect_pending_from_transcript_path(
    transcript_path: &Path,
    metadata: &HookMetadata,
    identity: CursorTranscriptIdentity,
    watermark_dir: Option<&Path>,
) -> anyhow::Result<CursorCollectResult> {
    let file_metadata = match fs::metadata(transcript_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CursorCollectResult::Empty(
                CursorEmptyReason::TranscriptPathMissing,
            ));
        }
        Err(_) => {
            return Ok(CursorCollectResult::Empty(
                CursorEmptyReason::TranscriptUnreadable,
            ));
        }
    };
    if !file_metadata.is_file() {
        return Ok(CursorCollectResult::Empty(
            CursorEmptyReason::TranscriptPathMissing,
        ));
    }
    let file_len = file_metadata.len();
    let modified_unix_ms = file_metadata
        .modified()
        .ok()
        .map(system_time_unix_ms)
        .unwrap_or(0);
    let content = match fs::read_to_string(transcript_path) {
        Ok(content) => content,
        Err(_) => {
            return Ok(CursorCollectResult::Empty(
                CursorEmptyReason::TranscriptUnreadable,
            ));
        }
    };
    let transcript_path_hash = stable_digest(transcript_path.to_string_lossy().as_bytes());
    let watermark_path = watermark_path_for_key(&identity.record_key, watermark_dir)?;
    let valid_primary_watermark = read_watermark(&watermark_path).and_then(|value| {
        valid_watermark_for_transcript(
            &value,
            &transcript_path_hash,
            &content,
            file_len,
            modified_unix_ms,
        )
    });
    let legacy_watermark_path =
        legacy_watermark_path_for_metadata(metadata, &watermark_path, watermark_dir)?;
    let mut legacy_watermark = legacy_watermark_path
        .as_ref()
        .and_then(|path| read_watermark(path));
    if let (Some(path), Some(watermark)) = (&legacy_watermark_path, legacy_watermark.as_ref()) {
        if watermark.version == 1
            && watermark.matches_transcript_path_hash(&transcript_path_hash)
            && watermark.transcript_path.is_some()
        {
            let sanitized = watermark.without_raw_path(&transcript_path_hash);
            write_watermark(path, &sanitized)?;
            legacy_watermark = Some(sanitized);
        }
    }
    let valid_legacy_watermark = legacy_watermark.and_then(|value| {
        valid_watermark_for_transcript(
            &value,
            &transcript_path_hash,
            &content,
            file_len,
            modified_unix_ms,
        )
    });
    let legacy_cleanup_path = valid_legacy_watermark
        .as_ref()
        .and(legacy_watermark_path.clone());
    let valid_watermark = valid_primary_watermark
        .clone()
        .or_else(|| valid_legacy_watermark.clone());

    if valid_watermark
        .as_ref()
        .is_some_and(|value| value.committed_offset == file_len)
    {
        let watermark = valid_watermark.as_ref().unwrap();
        if valid_primary_watermark.is_none()
            || watermark.transcript_path.is_some()
            || watermark.transcript_path_hash != transcript_path_hash
        {
            write_watermark(
                &watermark_path,
                &watermark.for_current_transcript(
                    &transcript_path_hash,
                    &content,
                    file_len,
                    modified_unix_ms,
                )?,
            )?;
        }
        if let Some(path) = legacy_cleanup_path.as_ref() {
            remove_watermark_if_exists(path)?;
        }
        return Ok(CursorCollectResult::Empty(
            CursorEmptyReason::WatermarkAtEof,
        ));
    }

    let source = TokenSourceKind::Cursor;
    let selection = if let Some(watermark) = valid_watermark {
        if !content.is_char_boundary(watermark.committed_offset as usize) {
            latest_round_selection(&content, file_len)?
        } else {
            appended_selection(&content, watermark.committed_offset)?
        }
    } else {
        latest_round_selection(&content, file_len)?
    };

    let Some(selection) = selection else {
        let reason = if parse_complete_rows(&content, 0)?.0.is_empty() {
            CursorEmptyReason::NoCompleteJsonlRows
        } else {
            CursorEmptyReason::NoNewCompleteRound
        };
        return Ok(CursorCollectResult::Empty(reason));
    };

    let total_tokens = selection.input_tokens + selection.output_tokens;
    let observed_at = metadata
        .timestamp
        .as_deref()
        .and_then(parse_timestamp)
        .or_else(|| file_metadata.modified().ok().map(DateTime::<Utc>::from))
        .unwrap_or_else(Utc::now);
    let source_record_id = format!(
        "{}:{}-{}",
        identity.record_key, selection.start_offset, selection.end_offset
    );
    let token_payload_hash = stable_hash(&json!({
        "source_record_id": source_record_id,
        "adapter_version": source.adapter_version(),
        "start_offset": selection.start_offset,
        "end_offset": selection.end_offset,
        "input_tokens": selection.input_tokens,
        "output_tokens": selection.output_tokens,
        "cached_input_tokens": 0,
        "cache_creation_input_tokens": 0,
        "reasoning_output_tokens": 0,
        "total_tokens": total_tokens,
    }))?;

    let observation = NormalizedObservation {
        source: source.as_str().to_string(),
        adapter_version: source.adapter_version().to_string(),
        source_record_id,
        source_record_id_confidence: SourceRecordIdConfidence::Fallback,
        session_id: identity.session_id,
        turn_id: identity.turn_id,
        turn_boundary_id: identity.turn_boundary_id,
        source_path: Some(identity.source_path),
        line_no: None,
        byte_offset: Some(selection.start_offset as i64),
        input_tokens: selection.input_tokens,
        output_tokens: selection.output_tokens,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens,
        cumulative_total_tokens: None,
        model: metadata.model.clone(),
        cwd: metadata.cwd.clone(),
        observed_at,
        token_payload_hash,
    };

    Ok(CursorCollectResult::Pending(PendingCursorCollection {
        observation,
        watermark_path,
        watermark: CursorWatermark {
            version: 1,
            transcript_path_hash,
            transcript_path: None,
            committed_prefix_hash: stable_digest(
                &content.as_bytes()[..selection.end_offset as usize],
            ),
            len: file_len,
            modified_unix_ms,
            committed_offset: selection.end_offset,
        },
        legacy_watermark_path: legacy_cleanup_path,
    }))
}

fn validate_conversation_id(conversation_id: &str) -> anyhow::Result<()> {
    let mut components = Path::new(conversation_id).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(segment)), None)
            if !segment.is_empty() && segment == OsStr::new(conversation_id) =>
        {
            Ok(())
        }
        _ => anyhow::bail!("invalid cursor conversation_id"),
    }
}

fn latest_round_selection(content: &str, file_len: u64) -> anyhow::Result<Option<TokenSelection>> {
    let (rows, safe_offset) = parse_complete_rows(content, 0)?;
    for user_index in (0..rows.len()).rev() {
        if !matches!(rows[user_index].role, CursorRole::User) {
            continue;
        }
        if let Some(selection) = completed_round_from_user(&rows, user_index) {
            return Ok(Some(TokenSelection {
                end_offset: selection.end_offset.min(safe_offset.min(file_len)),
                ..selection
            }));
        }
    }
    Ok(None)
}

fn appended_selection(
    content: &str,
    committed_offset: u64,
) -> anyhow::Result<Option<TokenSelection>> {
    let (rows, safe_offset) = parse_complete_rows(content, committed_offset as usize)?;
    if safe_offset as u64 == committed_offset {
        return Ok(None);
    }
    let Some(selection) = completed_appended_rounds(&rows, committed_offset) else {
        return Ok(None);
    };

    Ok(Some(selection))
}

fn completed_round_from_user(rows: &[CursorRow], user_index: usize) -> Option<TokenSelection> {
    let user = rows.get(user_index)?;
    let mut output_tokens = 0;
    let mut end_offset = None;
    for row in rows.iter().skip(user_index + 1) {
        if matches!(row.role, CursorRole::User) {
            break;
        }
        if matches!(row.role, CursorRole::Assistant) {
            output_tokens += row.tokens;
            end_offset = Some(row.end_offset);
        }
    }
    if user.tokens == 0 || output_tokens == 0 {
        return None;
    }
    Some(TokenSelection {
        input_tokens: user.tokens,
        output_tokens,
        start_offset: user.offset,
        end_offset: end_offset?,
    })
}

fn completed_appended_rounds(rows: &[CursorRow], committed_offset: u64) -> Option<TokenSelection> {
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut completed_end_offset = None;
    let mut current_user = None::<i64>;
    let mut current_output = 0;
    let mut current_end_offset = None;

    for row in rows {
        match row.role {
            CursorRole::User => {
                if let Some(user_tokens) = current_user.take() {
                    if current_output > 0 {
                        input_tokens += user_tokens;
                        output_tokens += current_output;
                        completed_end_offset = current_end_offset;
                    }
                }
                current_user = Some(row.tokens);
                current_output = 0;
                current_end_offset = None;
            }
            CursorRole::Assistant => {
                if current_user.is_some() {
                    current_output += row.tokens;
                    current_end_offset = Some(row.end_offset);
                }
            }
        }
    }

    if let Some(user_tokens) = current_user {
        if current_output > 0 {
            input_tokens += user_tokens;
            output_tokens += current_output;
            completed_end_offset = current_end_offset;
        }
    }

    if input_tokens == 0 || output_tokens == 0 {
        return None;
    }
    Some(TokenSelection {
        input_tokens,
        output_tokens,
        start_offset: committed_offset,
        end_offset: completed_end_offset?,
    })
}

fn parse_complete_rows(
    content: &str,
    start_offset: usize,
) -> anyhow::Result<(Vec<CursorRow>, u64)> {
    if start_offset > content.len() || !content.is_char_boundary(start_offset) {
        return Ok((Vec::new(), start_offset as u64));
    }

    let bytes = content.as_bytes();
    let mut offset = start_offset;
    let mut rows = Vec::new();
    while offset < bytes.len() {
        let Some(relative_newline) = bytes[offset..].iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let newline_offset = offset + relative_newline;
        let line_end = if newline_offset > offset && bytes[newline_offset - 1] == b'\r' {
            newline_offset - 1
        } else {
            newline_offset
        };
        let line = &content[offset..line_end];
        if !line.trim().is_empty() {
            if let Some(row) = parse_cursor_row(line, offset as u64, (newline_offset + 1) as u64)? {
                rows.push(row);
            }
        }
        offset = newline_offset + 1;
    }

    Ok((rows, offset as u64))
}

fn parse_cursor_row(line: &str, offset: u64, end_offset: u64) -> anyhow::Result<Option<CursorRow>> {
    let row: Value = serde_json::from_str(line)?;
    let role = match row.get("role").and_then(Value::as_str) {
        Some("user") => CursorRole::User,
        Some("assistant") => CursorRole::Assistant,
        _ => return Ok(None),
    };
    let tokens = estimate_message_tokens(row.pointer("/message/content"));
    Ok(Some(CursorRow {
        role,
        offset,
        end_offset,
        tokens,
    }))
}

fn estimate_message_tokens(content: Option<&Value>) -> i64 {
    let Some(content) = content else {
        return 0;
    };
    match content {
        Value::String(text) => estimate_text_tokens(text),
        Value::Array(items) => items
            .iter()
            .filter(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == "text")
            })
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(estimate_text_tokens)
            .sum(),
        _ => 0,
    }
}

fn estimate_text_tokens(text: &str) -> i64 {
    text.len().saturating_add(3).saturating_div(4) as i64
}

fn watermark_path_for_key(key: &str, watermark_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    let dir = match watermark_dir {
        Some(path) => path.to_path_buf(),
        None => dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("home directory not found"))?
            .join(".token-fire")
            .join("watermarks")
            .join("cursor"),
    };
    Ok(dir.join(format!("{}.json", stable_digest(key.as_bytes()))))
}

#[allow(dead_code)]
fn watermark_path(conversation_id: &str, watermark_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    watermark_path_for_key(conversation_id, watermark_dir)
}

fn legacy_watermark_path_for_metadata(
    metadata: &HookMetadata,
    current_watermark_path: &Path,
    watermark_dir: Option<&Path>,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(conversation_id) = metadata.conversation_id.as_deref() else {
        return Ok(None);
    };
    if validate_conversation_id(conversation_id).is_err() {
        return Ok(None);
    }
    let legacy_path = watermark_path(conversation_id, watermark_dir)?;
    if legacy_path == current_watermark_path {
        Ok(None)
    } else {
        Ok(Some(legacy_path))
    }
}

fn valid_watermark_for_transcript(
    watermark: &CursorWatermark,
    transcript_path_hash: &str,
    content: &str,
    file_len: u64,
    modified_unix_ms: i64,
) -> Option<CursorWatermark> {
    (watermark.version == 1)
        .then_some(watermark)
        .filter(|value| value.matches_transcript_path_hash(transcript_path_hash))
        .filter(|value| value.matches_file_content(content, file_len, modified_unix_ms))
        .cloned()
}

fn read_watermark(path: &Path) -> Option<CursorWatermark> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_watermark(path: &Path, watermark: &CursorWatermark) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(watermark)?)?;
    Ok(())
}

fn remove_watermark_if_exists(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn system_time_unix_ms(value: SystemTime) -> i64 {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
        Err(error) => -(error.duration().as_millis().min(i64::MAX as u128) as i64),
    }
}

fn stable_hash(value: &Value) -> anyhow::Result<String> {
    let canonical = canonical_json(value);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(stable_digest(&bytes))
}

fn stable_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in map {
                sorted.insert(key.clone(), canonical_json(value));
            }
            Value::Object(sorted.into_iter().collect::<Map<String, Value>>())
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::source::TokenSourceKind;
    use crate::core::observation::SourceRecordIdConfidence;

    fn write_cursor_fixture(root: &Path, project: &str, conversation_id: &str, body: &str) {
        let transcript_dir = root
            .join(".cursor/projects")
            .join(project)
            .join("agent-transcripts")
            .join(conversation_id);
        std::fs::create_dir_all(&transcript_dir).unwrap();
        std::fs::write(
            transcript_dir.join(format!("{conversation_id}.jsonl")),
            body,
        )
        .unwrap();
    }

    fn cursor_row(role: &str, text: &str) -> String {
        format!(
            r#"{{"role":"{role}","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    #[test]
    fn cursor_first_collection_reads_latest_round_only_and_marks_estimate() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        write_cursor_fixture(
            dir.path(),
            "project-a",
            "cursor-conv-1",
            include_str!("../../../tests/fixtures/cursor-transcript.jsonl"),
        );
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some("cursor-conv-1".to_string()),
            cwd: Some("/Users/example/cursor-project".to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        let observation = collect_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap()
        .expect("observation");

        assert_eq!(observation.source, TokenSourceKind::Cursor.as_str());
        assert_eq!(
            observation.adapter_version,
            TokenSourceKind::Cursor.adapter_version()
        );
        assert_eq!(observation.adapter_version, "cursor-storage-estimate-v1");
        assert_eq!(
            observation.source_record_id_confidence,
            SourceRecordIdConfidence::Fallback
        );
        assert_eq!(observation.session_id.as_deref(), Some("cursor-conv-1"));
        assert_eq!(observation.turn_id.as_deref(), Some("cursor-conv-1"));
        assert_eq!(observation.input_tokens, 8);
        assert_eq!(observation.output_tokens, 9);
        let expected_start_offset = include_str!("../../../tests/fixtures/cursor-transcript.jsonl")
            .lines()
            .take(2)
            .map(|line| line.len() + 1)
            .sum::<usize>() as i64;
        assert_eq!(observation.byte_offset, Some(expected_start_offset));
        assert_eq!(
            observation.total_tokens,
            observation.input_tokens + observation.output_tokens
        );
        let debug = format!("{observation:?}");
        assert!(!debug.contains("SENTINEL_CURSOR_USER"));
        assert!(!debug.contains("SENTINEL_CURSOR_ASSISTANT"));
    }

    #[test]
    fn cursor_repeated_collection_without_append_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        write_cursor_fixture(
            dir.path(),
            "project-a",
            "cursor-conv-1",
            include_str!("../../../tests/fixtures/cursor-transcript.jsonl"),
        );
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some("cursor-conv-1".to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        assert!(collect_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path())
        )
        .unwrap()
        .is_some());
        assert!(collect_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path())
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn cursor_watermark_does_not_persist_raw_transcript_path() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let conversation_id = "cursor-conv-privacy";
        write_cursor_fixture(
            dir.path(),
            "project-private-path",
            conversation_id,
            include_str!("../../../tests/fixtures/cursor-transcript.jsonl"),
        );
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some(conversation_id.to_string()),
            cwd: Some("/Users/example/cursor-project".to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        let observation = collect_for_conversation(
            conversation_id,
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap()
        .expect("observation");

        let source_path = observation.source_path.as_deref().unwrap_or_default();
        assert!(!source_path.contains(dir.path().to_string_lossy().as_ref()));
        assert!(!source_path.contains(".cursor"));
        assert!(!source_path.contains("project-private-path"));
        assert!(!source_path.contains("agent-transcripts"));

        let watermark_files = std::fs::read_dir(watermark.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(watermark_files.len(), 1);
        let watermark_json = std::fs::read_to_string(watermark_files[0].path()).unwrap();
        let watermark_value: serde_json::Value = serde_json::from_str(&watermark_json).unwrap();

        assert!(watermark_value.get("transcript_path_hash").is_some());
        assert!(watermark_value.get("transcript_path").is_none());
        assert!(!watermark_json.contains(dir.path().to_string_lossy().as_ref()));
        assert!(!watermark_json.contains(".cursor"));
        assert!(!watermark_json.contains("project-private-path"));
        assert!(!watermark_json.contains("agent-transcripts"));
        assert!(!watermark_json.contains(conversation_id));
    }

    #[test]
    fn cursor_path_first_migrates_legacy_conversation_watermark_without_raw_path() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let conversation_id = "cursor-conv-legacy-privacy";
        let transcript_path = dir.path().join("cursor-legacy-transcript.jsonl");
        let content = include_str!("../../../tests/fixtures/cursor-transcript.jsonl");
        std::fs::write(&transcript_path, content).unwrap();
        let file_metadata = std::fs::metadata(&transcript_path).unwrap();
        let file_len = file_metadata.len();
        let modified_unix_ms = file_metadata
            .modified()
            .ok()
            .map(system_time_unix_ms)
            .unwrap_or(0);
        let legacy_watermark_path =
            watermark_path(conversation_id, Some(watermark.path())).unwrap();
        std::fs::create_dir_all(legacy_watermark_path.parent().unwrap()).unwrap();
        std::fs::write(
            &legacy_watermark_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "transcript_path_hash": "",
                "transcript_path": transcript_path.to_string_lossy(),
                "committed_prefix_hash": stable_digest(content.as_bytes()),
                "len": file_len,
                "modified_unix_ms": modified_unix_ms,
                "committed_offset": file_len,
            }))
            .unwrap(),
        )
        .unwrap();
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some(conversation_id.to_string()),
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };
        let identity = CursorTranscriptIdentity::from_path(&transcript_path, &metadata);
        let new_watermark_path =
            watermark_path_for_key(&identity.record_key, Some(watermark.path())).unwrap();

        let result = collect_pending_from_transcript_path(
            &transcript_path,
            &metadata,
            identity,
            Some(watermark.path()),
        )
        .unwrap();

        assert!(matches!(
            result,
            CursorCollectResult::Empty(CursorEmptyReason::WatermarkAtEof)
        ));
        assert!(
            !legacy_watermark_path.exists(),
            "legacy raw-path watermark should be migrated away"
        );
        let migrated_json = std::fs::read_to_string(new_watermark_path).unwrap();
        let migrated_value: serde_json::Value = serde_json::from_str(&migrated_json).unwrap();
        assert!(migrated_value.get("transcript_path_hash").is_some());
        assert!(migrated_value.get("transcript_path").is_none());
        assert!(!migrated_json.contains(dir.path().to_string_lossy().as_ref()));
        assert!(!migrated_json.contains("cursor-legacy-transcript.jsonl"));
        assert!(!migrated_json.contains(conversation_id));
    }

    #[test]
    fn cursor_rejects_conversation_ids_that_are_not_single_path_segment() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            ..HookMetadata::default()
        };

        for conversation_id in ["", ".", "..", "../x", "a/b", "/tmp/x"] {
            assert!(
                resolve_cursor_transcript(conversation_id, Some(dir.path())).is_none(),
                "resolver accepted invalid conversation_id: {conversation_id:?}"
            );
            let error = collect_for_conversation(
                conversation_id,
                &metadata,
                Some(dir.path()),
                Some(watermark.path()),
            )
            .expect_err("invalid conversation_id should be rejected");
            assert!(
                error.to_string().contains("invalid cursor conversation_id"),
                "unexpected error for {conversation_id:?}: {error}"
            );
        }
    }

    #[test]
    fn cursor_resolver_ignores_nested_false_positive() {
        let dir = tempfile::tempdir().unwrap();
        let false_positive = dir
            .path()
            .join(".cursor/projects/project-a/not-agent-transcripts/cursor-conv-1");
        std::fs::create_dir_all(&false_positive).unwrap();
        std::fs::write(false_positive.join("cursor-conv-1.jsonl"), "{}").unwrap();

        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some("cursor-conv-1".to_string()),
            ..HookMetadata::default()
        };

        assert!(
            collect_for_conversation("cursor-conv-1", &metadata, Some(dir.path()), None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cursor_appended_collection_reads_only_new_completed_round() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let first_body = r#"{"role":"user","message":{"content":[{"type":"text","text":"SENTINEL_CURSOR_USER_ROUND_ONE"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"SENTINEL_CURSOR_ASSISTANT_ROUND_ONE"}]}}
"#;
        write_cursor_fixture(dir.path(), "project-a", "cursor-conv-1", first_body);
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some("cursor-conv-1".to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        assert!(collect_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path())
        )
        .unwrap()
        .is_some());

        let appended_body = format!(
            "{first_body}{}",
            r#"{"role":"user","message":{"content":[{"type":"text","text":"SENTINEL_CURSOR_USER_ROUND_TWO"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"SENTINEL_CURSOR_ASSISTANT_ROUND_TWO"}]}}
"#
        );
        write_cursor_fixture(dir.path(), "project-a", "cursor-conv-1", &appended_body);

        let observation = collect_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap()
        .expect("appended observation");

        assert_eq!(observation.input_tokens, 8);
        assert_eq!(observation.output_tokens, 9);
        assert_eq!(observation.byte_offset, Some(first_body.len() as i64));
        assert_eq!(observation.total_tokens, 17);
        assert!(observation.source_record_id.starts_with("cursor:"));
        assert!(!observation.source_record_id.contains("cursor-conv-1"));
    }

    #[test]
    fn cursor_valid_tail_without_newline_waits_until_committed() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let conversation_id = "cursor-conv-no-newline";
        let complete_user = cursor_row("user", "ROUND_ONE_USER");
        let complete_assistant = cursor_row("assistant", "ROUND_ONE_ASSISTANT");
        let tail_user = cursor_row("user", "ROUND_TWO_USER");
        let tail_assistant = cursor_row("assistant", "ROUND_TWO_ASSISTANT");
        let first_body =
            format!("{complete_user}\n{complete_assistant}\n{tail_user}\n{tail_assistant}");
        write_cursor_fixture(dir.path(), "project-a", conversation_id, &first_body);
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some(conversation_id.to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        let first = collect_for_conversation(
            conversation_id,
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap()
        .expect("first committed observation");

        let expected_first_end = complete_user.len() + 1 + complete_assistant.len() + 1;
        assert_eq!(first.byte_offset, Some(0));
        assert!(first.source_record_id.starts_with("cursor:"));
        assert!(first
            .source_record_id
            .ends_with(&format!(":0-{expected_first_end}")));
        assert!(!first.source_record_id.contains(conversation_id));

        write_cursor_fixture(
            dir.path(),
            "project-a",
            conversation_id,
            &format!("{first_body}\n"),
        );
        let second = collect_for_conversation(
            conversation_id,
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap()
        .expect("tail counted after newline");

        assert_eq!(second.byte_offset, Some(expected_first_end as i64));
        assert!(second.total_tokens > 0);
    }

    #[test]
    fn cursor_same_path_rewrite_discards_old_watermark() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let conversation_id = "cursor-conv-rewrite";
        let first_body = format!(
            "{}\n{}\n",
            cursor_row("user", "ORIGINAL_USER"),
            cursor_row("assistant", "ORIGINAL_ASSISTANT")
        );
        write_cursor_fixture(dir.path(), "project-a", conversation_id, &first_body);
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some(conversation_id.to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        let original = collect_for_conversation(
            conversation_id,
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap()
        .expect("original observation");

        let replacement_body = format!(
            "{}\n{}\n",
            cursor_row("user", "REPLACED_USER_TEXT_WITH_DIFFERENT_PREFIX"),
            cursor_row("assistant", "REPLACED_ASSISTANT_TEXT")
        );
        write_cursor_fixture(dir.path(), "project-a", conversation_id, &replacement_body);

        let replacement = collect_for_conversation(
            conversation_id,
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap()
        .expect("replacement latest round");

        assert_eq!(replacement.byte_offset, Some(0));
        assert_ne!(replacement.source_record_id, original.source_record_id);
    }

    #[test]
    fn cursor_pending_collection_can_skip_commit_after_failed_ingest() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        write_cursor_fixture(
            dir.path(),
            "project-a",
            "cursor-conv-1",
            include_str!("../../../tests/fixtures/cursor-transcript.jsonl"),
        );
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            conversation_id: Some("cursor-conv-1".to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        let pending = collect_pending_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap();
        let pending = match pending {
            CursorCollectResult::Pending(pending) => pending,
            CursorCollectResult::Empty(reason) => panic!("expected pending, got {reason:?}"),
        };
        let source_record_id = pending.observation().source_record_id.clone();

        drop(pending);

        let retry = collect_pending_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap();
        let retry = match retry {
            CursorCollectResult::Pending(pending) => pending,
            CursorCollectResult::Empty(reason) => {
                panic!("expected retry pending observation after skipped commit, got {reason:?}")
            }
        };
        assert_eq!(retry.observation().source_record_id, source_record_id);

        retry.commit().unwrap();
        let after_commit = collect_pending_for_conversation(
            "cursor-conv-1",
            &metadata,
            Some(dir.path()),
            Some(watermark.path()),
        )
        .unwrap();
        assert!(matches!(
            after_commit,
            CursorCollectResult::Empty(CursorEmptyReason::WatermarkAtEof)
        ));
    }

    #[test]
    fn cursor_collects_from_explicit_transcript_path_without_cursor_projects() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let transcript_path = dir.path().join("cursor-real-transcript.jsonl");
        std::fs::write(
            &transcript_path,
            include_str!("../../../tests/fixtures/cursor-transcript.jsonl"),
        )
        .unwrap();
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            session_id: Some("cursor-session-path".to_string()),
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            conversation_id: Some("cursor-conv-path".to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        let identity = CursorTranscriptIdentity::from_path(&transcript_path, &metadata);
        let result = collect_pending_from_transcript_path(
            &transcript_path,
            &metadata,
            identity,
            Some(watermark.path()),
        )
        .unwrap();

        let pending = match result {
            CursorCollectResult::Pending(pending) => pending,
            CursorCollectResult::Empty(reason) => panic!("expected pending, got {reason:?}"),
        };
        let observation = pending.observation();
        assert_eq!(observation.source, TokenSourceKind::Cursor.as_str());
        assert_eq!(
            observation.session_id.as_deref(),
            Some("cursor-session-path")
        );
        assert_eq!(
            observation.source_record_id_confidence,
            SourceRecordIdConfidence::Fallback
        );
        assert!(observation
            .source_path
            .as_deref()
            .unwrap()
            .starts_with("cursor:"));
        assert!(!observation
            .source_path
            .as_deref()
            .unwrap()
            .contains(dir.path().to_string_lossy().as_ref()));
        assert!(observation.source_record_id.starts_with("cursor:"));
        assert!(observation.total_tokens > 0);
    }

    #[test]
    fn cursor_path_first_repeat_without_append_reports_watermark_at_eof() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let transcript_path = dir.path().join("cursor-repeat.jsonl");
        std::fs::write(
            &transcript_path,
            include_str!("../../../tests/fixtures/cursor-transcript.jsonl"),
        )
        .unwrap();
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            session_id: Some("cursor-session-repeat".to_string()),
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        let first = collect_pending_from_transcript_path(
            &transcript_path,
            &metadata,
            CursorTranscriptIdentity::from_path(&transcript_path, &metadata),
            Some(watermark.path()),
        )
        .unwrap();
        match first {
            CursorCollectResult::Pending(pending) => pending.commit().unwrap(),
            CursorCollectResult::Empty(reason) => panic!("expected pending, got {reason:?}"),
        }

        let second = collect_pending_from_transcript_path(
            &transcript_path,
            &metadata,
            CursorTranscriptIdentity::from_path(&transcript_path, &metadata),
            Some(watermark.path()),
        )
        .unwrap();

        match second {
            CursorCollectResult::Empty(reason) => {
                assert_eq!(reason, CursorEmptyReason::WatermarkAtEof);
            }
            CursorCollectResult::Pending(_) => panic!("expected watermark-at-eof empty result"),
        }
    }

    #[test]
    fn cursor_path_first_appended_round_uses_same_path_watermark() {
        let dir = tempfile::tempdir().unwrap();
        let watermark = tempfile::tempdir().unwrap();
        let transcript_path = dir.path().join("cursor-append.jsonl");
        let first_body = format!(
            "{}\n{}\n",
            cursor_row("user", "ROUND_ONE_USER"),
            cursor_row("assistant", "ROUND_ONE_ASSISTANT")
        );
        std::fs::write(&transcript_path, &first_body).unwrap();
        let metadata = HookMetadata {
            source: Some("cursor".to_string()),
            hook_event_name: Some("stop".to_string()),
            session_id: Some("cursor-session-append".to_string()),
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            timestamp: Some("2026-07-05T11:00:00Z".to_string()),
            ..HookMetadata::default()
        };

        match collect_pending_from_transcript_path(
            &transcript_path,
            &metadata,
            CursorTranscriptIdentity::from_path(&transcript_path, &metadata),
            Some(watermark.path()),
        )
        .unwrap()
        {
            CursorCollectResult::Pending(pending) => pending.commit().unwrap(),
            CursorCollectResult::Empty(reason) => panic!("expected first pending, got {reason:?}"),
        }

        let appended_body = format!(
            "{first_body}{}\n{}\n",
            cursor_row("user", "ROUND_TWO_USER"),
            cursor_row("assistant", "ROUND_TWO_ASSISTANT")
        );
        std::fs::write(&transcript_path, appended_body).unwrap();

        let second = collect_pending_from_transcript_path(
            &transcript_path,
            &metadata,
            CursorTranscriptIdentity::from_path(&transcript_path, &metadata),
            Some(watermark.path()),
        )
        .unwrap();

        let pending = match second {
            CursorCollectResult::Pending(pending) => pending,
            CursorCollectResult::Empty(reason) => {
                panic!("expected appended pending, got {reason:?}")
            }
        };
        assert_eq!(
            pending.observation().byte_offset,
            Some(first_body.len() as i64)
        );
        assert!(pending.observation().total_tokens > 0);
    }
}
