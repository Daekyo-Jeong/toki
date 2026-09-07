use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct LogLine {
    pub uuid: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub timestamp: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<Message>,
}

#[derive(Deserialize, Debug)]
pub struct Message {
    pub role: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
    /// §3.3.5 ①: raw content blocks (tool_use lives here). Kept as Value —
    /// content is either a string or a block array depending on line type,
    /// and the live retry feed only pokes at tool_use name/file_path.
    #[serde(default)]
    pub content: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug, Default)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: i64,
}

pub fn parse_line(line: &str) -> Option<LogLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str::<LogLine>(trimmed).ok()
}
