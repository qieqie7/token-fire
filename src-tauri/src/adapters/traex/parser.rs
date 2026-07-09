use std::path::Path;

use crate::adapters::source::SourceContext;
use crate::adapters::transcript::{ParseReport, TranscriptParser};

#[derive(Debug, Default, Clone)]
pub struct TraexParser;

impl TraexParser {
    pub fn parse_str(&self, path: &Path, content: &str) -> anyhow::Result<ParseReport> {
        TranscriptParser::new(SourceContext::traex()).parse_str(path, content)
    }
}
