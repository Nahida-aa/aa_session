//! v1 schema 的结构体（与 [`crate::v1`] 的行为分离，便于单测）。

use serde::Deserialize;

/// v1 `session` 表的一行（只取下游需要的列，避免把 27 列全拉过来）。
#[derive(Debug, Clone, Deserialize)]
pub struct V1SessionRow {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub directory: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub time_created: i64,
    pub time_updated: i64,
    #[serde(default)]
    pub tokens_input: i64,
    #[serde(default)]
    pub tokens_output: i64,
    #[serde(default)]
    pub tokens_reasoning: i64,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub summary_files: i64,
    #[serde(default)]
    pub time_archived: Option<i64>,
}

/// v1 `message` 行里**只关心顶层 role**——正文在 `part` 表里，不在这。
#[derive(Debug, Deserialize)]
pub struct V1MessageMeta {
    pub role: String,
}

/// `part.data` 的顶层形态。
///
/// 实测（2026-10）的 `type` 取值域：
/// `text` / `tool` / `reasoning` / `patch` / `compaction` / `step-start` /
/// `step-finish`。未知类型归到 `PartBlock::Other`，不丢数据。
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum PartBlock {
    /// `data: {"text": "...", ...}`
    #[serde(rename = "text")]
    Text {
        #[serde(default)]
        text: String,
    },

    /// `data: {"text": "...", ...}`
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        text: String,
    },

    /// `data: {"tool":"read","state":{"status":"completed","input":{...},"content":[...]}}`
    #[serde(rename = "tool")]
    Tool {
        #[serde(default)]
        tool: String,
        #[serde(default)]
        state: Option<V1ToolState>,
    },

    /// `data: {"patch":{"files":[...]}}`
    #[serde(rename = "patch")]
    Patch {
        #[serde(default)]
        patch: Option<V1Patch>,
    },

    /// `data: {"summary":"...", "reason":"auto"}`
    #[serde(rename = "compaction")]
    Compaction {
        #[serde(default)]
        summary: Option<String>,
    },

    /// `step-start` / `step-finish` / 未知类型。
    #[serde(untagged)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct V1ToolState {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    /// 输出：`[{"type":"text","text":"..."}, ...]`，与 v2 同构。
    #[serde(default)]
    pub content: Option<Vec<V1TextContent>>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct V1TextContent {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct V1Patch {
    #[serde(default)]
    pub files: Vec<V1PatchFile>,
}

#[derive(Debug, Deserialize)]
pub struct V1PatchFile {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub patch: String,
}

/// 把 v1 `part.data` 解析成一个 [`PartBlock`]；失败时原样保留 JSON，不丢数据。
pub fn parse_part(data: &str) -> PartBlock {
    serde_json::from_str(data).unwrap_or(PartBlock::Other)
}
