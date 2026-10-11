//! 把 opencode `session_message.data` 里的 JSON 块翻译成 [`ContentBlock`]。
//!
//! opencode 的 `data` 列形态**因 `type` 而异**，不能统一按 JSON 解析：
//!
//! | `session_message.type` | `data` 形态 |
//! |---|---|
//! | `user` | `{"text": "...", "time": {...}}` |
//! | `assistant` | `{"agent","model","content":[reasoning\|text\|tool, ...]}` |
//! | `compaction` | `{"status","reason","summary"}` |
//! | `synthetic` / `system` | `{"text": "...", "time": {...}}` |
//!
//! 未知 type 一律降级为 [`ContentBlock::Other`]，不报错 —— 上游加新类型时
//! 索引流程不该整体失败。

use aa_session_core::{ContentBlock, EntryKind};

/// 解析 opencode `session_message.type`。
///
/// 未知取值映射到 [`EntryKind::Synthetic`] 而不是报错 —— 上游加新类型时
/// 不该让整个工具挂掉。
pub fn entry_kind_from_type(s: &str) -> EntryKind {
    match s {
        "user" => EntryKind::User,
        "assistant" => EntryKind::Assistant,
        "compaction" => EntryKind::Compaction,
        "system" => EntryKind::System,
        _ => EntryKind::Synthetic,
    }
}

/// `assistant` 的 `content` 数组里单个块的原始 JSON。
type RawBlock = serde_json::Value;

/// 把一条 `session_message` 的 `data` 解析成分块列表。
///
/// 未识别的结构会退化成 [`ContentBlock::Other`]，保证信息不丢。
pub fn parse_blocks(kind: EntryKind, data: &str) -> Result<Vec<ContentBlock>, serde_json::Error> {
    // `user` / `synthetic` / `system` 是 `{"text": ...}` 形状。
    match kind {
        EntryKind::User | EntryKind::Synthetic | EntryKind::System => {
            let v: serde_json::Value = serde_json::from_str(data)?;
            let Some(text) = v.get("text").and_then(|t| t.as_str()) else {
                return Ok(vec![ContentBlock::Other(v)]);
            };
            Ok(vec![ContentBlock::Text(text.to_string())])
        }

        EntryKind::Assistant => {
            let v: serde_json::Value = serde_json::from_str(data)?;
            let Some(arr) = v.get("content").and_then(|c| c.as_array()) else {
                return Ok(vec![ContentBlock::Other(v)]);
            };
            Ok(arr.iter().map(parse_assistant_block).collect())
        }

        EntryKind::Compaction => {
            let v: serde_json::Value = serde_json::from_str(data)?;
            let Some(summary) = v.get("summary").and_then(|s| s.as_str()) else {
                return Ok(vec![ContentBlock::Other(v)]);
            };
            Ok(vec![ContentBlock::Text(summary.to_string())])
        }

        // `thought` / `tool` 不是 `session_message.type` 的取值，
        // 走不到这里；保留分支以穷尽匹配。
        EntryKind::Thought | EntryKind::ToolCall => {
            let v: serde_json::Value = serde_json::from_str(data)?;
            Ok(vec![ContentBlock::Other(v)])
        }
    }
}

/// 解析 `assistant.content[]` 里的单个块。
fn parse_assistant_block(block: &RawBlock) -> ContentBlock {
    match block.get("type").and_then(|t| t.as_str()) {
        Some("text") => match block.get("text").and_then(|t| t.as_str()) {
            Some(text) => ContentBlock::Text(text.to_string()),
            None => ContentBlock::Other(block.clone()),
        },

        Some("reasoning") => match block.get("text").and_then(|t| t.as_str()) {
            Some(text) => ContentBlock::Reasoning(text.to_string()),
            None => ContentBlock::Other(block.clone()),
        },

        Some("tool") => parse_tool_block(block),

        _ => ContentBlock::Other(block.clone()),
    }
}

/// 解析一个 tool 块。
///
/// 实测形态（2026-10，opencode 1.18.35）：
/// ```json
/// {"type":"tool","id":"call_...","name":"read",
///  "state":{"status":"completed","input":{...},
///            "content":[{"type":"text","text":"..."}],"metadata":{...}}}
/// ```
/// `status == "error"` 时 `state` 里没有 `content`，错误信息在 `state.error`。
fn parse_tool_block(block: &RawBlock) -> ContentBlock {
    let id = block
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = block
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let state = block.get("state");
    let status = state
        .and_then(|s| s.get("status"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let input = state.and_then(|s| s.get("input")).cloned();

    // output：优先拼接 `state.content[].text`；error 状态下退回 `state.error`。
    let output = state.and_then(|s| {
        if let Some(content) = s.get("content").and_then(|c| c.as_array()) {
            let text: Vec<&str> = content
                .iter()
                .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                .collect();
            if !text.is_empty() {
                return Some(serde_json::Value::String(text.join("\n")));
            }
        }
        s.get("error").cloned()
    });

    ContentBlock::ToolCall {
        id,
        name,
        status,
        input,
        output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_is_text_block() {
        let data = r#"{"text":"你好","time":{"created":123}}"#;
        let blocks = parse_blocks(EntryKind::User, data).unwrap();
        assert_eq!(blocks, vec![ContentBlock::Text("你好".into())]);
    }

    #[test]
    fn assistant_splits_reasoning_text_and_tool() {
        let data = r#"{"agent":"general","content":[
            {"type":"reasoning","text":"想一下"},
            {"type":"text","text":"答案"},
            {"type":"tool","id":"call_1","name":"read",
             "state":{"status":"completed","input":{"path":"a.rs"},
                      "content":[{"type":"text","text":"文件内容"}]}}
        ]}"#;
        let blocks = parse_blocks(EntryKind::Assistant, data).unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0], ContentBlock::Reasoning("想一下".into()));
        assert_eq!(blocks[1], ContentBlock::Text("答案".into()));
        match &blocks[2] {
            ContentBlock::ToolCall {
                name,
                status,
                input,
                output,
                ..
            } => {
                assert_eq!(name, "read");
                assert_eq!(status.as_deref(), Some("completed"));
                assert!(input.is_some());
                assert_eq!(output.as_ref(), Some(&serde_json::json!("文件内容")));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn tool_error_state_falls_back_to_error_field() {
        // 实测 error 状态的 tool 块没有 content，错误信息在 state.error。
        let data = r#"{"agent":"x","content":[
            {"type":"tool","id":"c","name":"bash","state":{"status":"error","input":{},
                     "error":"命令失败"}}
        ]}"#;
        let blocks = parse_blocks(EntryKind::Assistant, data).unwrap();
        match &blocks[0] {
            ContentBlock::ToolCall { status, output, .. } => {
                assert_eq!(status.as_deref(), Some("error"));
                assert_eq!(output.as_ref(), Some(&serde_json::json!("命令失败")));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn unknown_block_types_are_preserved_not_dropped() {
        let data = r#"{"content":[{"type":"future_thing","payload":42}]}"#;
        let blocks = parse_blocks(EntryKind::Assistant, data).unwrap();
        match &blocks[0] {
            ContentBlock::Other(v) => assert_eq!(v["payload"], 42),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn compaction_uses_summary_field() {
        let data = r###"{"status":"completed","reason":"auto","summary":"# 摘要"}"###;
        let blocks = parse_blocks(EntryKind::Compaction, data).unwrap();
        assert_eq!(blocks, vec![ContentBlock::Text("# 摘要".into())]);
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_blocks(EntryKind::User, "{not json").is_err());
    }
}
