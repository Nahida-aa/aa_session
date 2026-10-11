//! 把会话导出为可读 JSON。
//!
//! # 格式取舍
//!
//! 导出的 JSON 是**给人读的**，不是给程序反序列化回去的（v1 不做 import）。
//! 用户会在 Zed 里全文搜索它，所以：
//!
//! - **每条消息一个对象**，`text` 字段直接是纯文本，Zed 搜 `音频解码` 能
//!   直接命中字符串本身，而不必匹配到某个深层嵌套路径；
//! - tool 调用的 `input` / `output` **默认不导出**（一份完整会话导出后可达
//!   数百 MB，绝大多数是文件全文和命令输出），需要时用 `--with-tool-output`；
//! - 保留 `seq` 作为稳定标识，将来 fork 以它为边界；
//! - `app` 字段记录来源应用，导出的文件自带出处，不依赖外部上下文。

use aa_session_core::{EntryKind, MessageBatch, SessionMeta};
use serde::Serialize;

/// 导出选项。
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// 是否导出工具调用的 input / output。
    pub with_tool_output: bool,
    /// 是否导出推理（thinking）内容。
    pub with_reasoning: bool,
    /// 每条 tool 输出的最大字符数；`None` 表示不截断。
    pub max_tool_output_chars: Option<usize>,
}

/// 导出文件的顶层结构。
#[derive(Debug, Serialize)]
pub struct ExportFile {
    /// 格式版本，便于将来演进。
    pub format_version: u32,
    /// 产生该会话的 agent 应用名。
    pub app: String,
    /// 会话元信息。
    pub session: SessionMeta,
    /// 因 JSON 损坏而被跳过的消息数。
    pub skipped_messages: usize,
    /// 消息列表，按 `seq` 升序。
    pub messages: Vec<ExportMessage>,
}

/// 一条导出后的消息。
#[derive(Debug, Serialize)]
pub struct ExportMessage {
    /// 会话内顺序号。
    pub seq: i64,
    /// 消息 id。
    pub id: String,
    /// 角色。
    pub role: &'static str,
    /// 创建时间（Unix 毫秒）。
    pub time_created: i64,
    /// 该消息的纯文本正文（把所有可读分块拼起来）。
    ///
    /// 单独给出这个字段，是为了让 Zed 的全文搜索能直接命中裸文本，
    /// 而不必在 `blocks` 里逐层匹配。
    pub text: String,
    /// 结构化分块，保留完整信息。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<ExportBlock>,
}

/// 导出的一个内容分块。
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExportBlock {
    /// 文本。
    Text {
        /// 正文。
        text: String,
    },
    /// 推理内容。
    Reasoning {
        /// 正文。
        text: String,
    },
    /// 工具调用。
    ToolCall {
        /// 调用 id。
        id: String,
        /// 工具名。
        name: String,
        /// 状态。
        status: Option<String>,
        /// 入参 JSON（受 `--with-tool-output` 控制）。
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
        /// 输出 JSON（受 `--with-tool-output` 控制，且可能被截断）。
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
    },
    /// 未识别的块，原样保留。
    Other(serde_json::Value),
}

/// 组装导出文件。
pub fn build(session: &SessionMeta, batch: &MessageBatch, opts: &ExportOptions) -> ExportFile {
    let messages = batch
        .entries
        .iter()
        .map(|msg| build_message(msg, opts))
        .collect();

    ExportFile {
        format_version: 1,
        app: session.app.clone(),
        session: session.clone(),
        skipped_messages: batch.skipped,
        messages,
    }
}

/// 把一条消息转成导出格式。
fn build_message(msg: &aa_session_core::MessageEntry, opts: &ExportOptions) -> ExportMessage {
    let mut text_parts: Vec<String> = Vec::new();
    let mut blocks: Vec<ExportBlock> = Vec::new();

    for block in &msg.content {
        match block {
            aa_session_core::ContentBlock::Text(t) => {
                if !t.trim().is_empty() {
                    text_parts.push(t.clone());
                    blocks.push(ExportBlock::Text { text: t.clone() });
                }
            }

            aa_session_core::ContentBlock::Reasoning(t) => {
                if !t.trim().is_empty() {
                    if opts.with_reasoning {
                        text_parts.push(t.clone());
                    }
                    if opts.with_reasoning {
                        blocks.push(ExportBlock::Reasoning { text: t.clone() });
                    }
                }
            }

            aa_session_core::ContentBlock::ToolCall {
                id,
                name,
                status,
                input,
                output,
            } => {
                // tool 名总是放进正文，搜 `read` / `bash` 时有用。
                text_parts.push(format!("[tool:{name}]"));

                blocks.push(ExportBlock::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    status: status.clone(),
                    input: opts.with_tool_output.then(|| input.clone()).flatten(),
                    output: opts
                        .with_tool_output
                        .then(|| truncate(output, opts.max_tool_output_chars))
                        .flatten(),
                });
            }

            aa_session_core::ContentBlock::Diff { path, patch } => {
                if !patch.trim().is_empty() {
                    // diff 正文很有搜索价值（找"当时改了哪个函数"），
                    // 所以始终导出，但只带路径不带全文，除非要 tool output。
                    let body = if opts.with_tool_output {
                        format!("--- {path}\n{patch}")
                    } else {
                        format!("--- {path}")
                    };
                    text_parts.push(body);
                }
            }

            aa_session_core::ContentBlock::Other(_) => {
                // 未识别的块不塞进正文（可能是大对象），只在 blocks 里保留。
            }
        }
    }

    // 非人类可读的消息（纯 tool 调用）若正文为空，用占位符说明，
    // 免得用户在 JSON 里看到 `"text": ""` 一头雾水。
    let text = if text_parts.is_empty() {
        if msg.kind.is_readable() {
            String::new()
        } else {
            format!("[{} message, no readable content]", role_name(msg.kind))
        }
    } else {
        text_parts.join("\n\n")
    };

    ExportMessage {
        seq: msg.seq,
        id: msg.id.clone(),
        role: role_name(msg.kind),
        time_created: msg.time_created,
        text,
        blocks,
    }
}

/// 角色的稳定字符串形式。
fn role_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::User => "user",
        EntryKind::Assistant => "assistant",
        EntryKind::Thought => "thought",
        EntryKind::ToolCall => "tool",
        EntryKind::Compaction => "compaction",
        EntryKind::Synthetic => "synthetic",
        EntryKind::System => "system",
    }
}

/// 按字符数截断 JSON 值；超长时替换成带截断标记的字符串。
fn truncate(value: &Option<serde_json::Value>, max: Option<usize>) -> Option<serde_json::Value> {
    let value = value.as_ref()?;
    let max = match max {
        Some(m) => m,
        None => return Some(value.clone()),
    };

    // 只对字符串做截断；对象/数组整体转字符串会丢掉结构，
    // 但为了控制体积，这里也走同一条路径。
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    if text.chars().count() <= max {
        return Some(value.clone());
    }

    let truncated: String = text.chars().take(max).collect();
    Some(serde_json::Value::String(format!(
        "{truncated}\n... [truncated, {} chars total]",
        text.chars().count()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aa_session_core::{ContentBlock, MessageEntry};

    fn meta() -> SessionMeta {
        SessionMeta {
            app: "opencode".into(),
            id: "ses_x".into(),
            project_id: "p1".into(),
            project_path: "/tmp/proj".into(),
            parent_id: None,
            title: "标题".into(),
            directory: "/tmp/proj".into(),
            agent: Some("build".into()),
            model: None,
            time_created: 1,
            time_updated: 2,
            tokens_input: 0,
            tokens_output: 0,
            tokens_reasoning: 0,
            cost: None,
            files_changed: 0,
            archived_at: None,
        }
    }

    fn msg(seq: i64, kind: EntryKind, blocks: Vec<ContentBlock>) -> MessageEntry {
        MessageEntry {
            id: format!("m{seq}"),
            seq,
            kind,
            content: blocks,
            time_created: seq,
        }
    }

    #[test]
    fn text_block_becomes_searchable_text() {
        let batch = MessageBatch {
            entries: vec![msg(
                0,
                EntryKind::User,
                vec![ContentBlock::Text("音频解码的关键".into())],
            )],
            skipped: 0,
        };
        let f = build(&meta(), &batch, &ExportOptions::default());
        assert_eq!(f.messages[0].text, "音频解码的关键");
        assert_eq!(f.messages[0].role, "user");
    }

    #[test]
    fn tool_output_is_excluded_by_default_but_name_is_kept() {
        let batch = MessageBatch {
            entries: vec![msg(
                1,
                EntryKind::Assistant,
                vec![ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    status: Some("completed".into()),
                    input: Some(serde_json::json!({"cmd":"ls"})),
                    output: Some(serde_json::json!("SECRET-OUTPUT")),
                }],
            )],
            skipped: 0,
        };

        let f = build(&meta(), &batch, &ExportOptions::default());
        // 工具名进正文（搜 bash 有用），输出不进。
        assert_eq!(f.messages[0].text, "[tool:bash]");
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("SECRET-OUTPUT"), "默认不应导出工具输出");
        assert!(!json.contains("\"cmd\""), "默认不应导出工具入参");
    }

    #[test]
    fn tool_output_included_on_request() {
        let batch = MessageBatch {
            entries: vec![msg(
                1,
                EntryKind::Assistant,
                vec![ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    status: None,
                    input: None,
                    output: Some(serde_json::json!("SECRET-OUTPUT")),
                }],
            )],
            skipped: 0,
        };
        let opts = ExportOptions {
            with_tool_output: true,
            ..Default::default()
        };
        let f = build(&meta(), &batch, &opts);
        assert!(serde_json::to_string(&f).unwrap().contains("SECRET-OUTPUT"));
    }

    #[test]
    fn tool_output_is_truncated_when_capped() {
        let batch = MessageBatch {
            entries: vec![msg(
                1,
                EntryKind::Assistant,
                vec![ContentBlock::ToolCall {
                    id: "c".into(),
                    name: "read".into(),
                    status: None,
                    input: None,
                    output: Some(serde_json::json!("x".repeat(500))),
                }],
            )],
            skipped: 0,
        };
        let opts = ExportOptions {
            with_tool_output: true,
            max_tool_output_chars: Some(10),
            ..Default::default()
        };
        let f = build(&meta(), &batch, &opts);
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("truncated"), "应标记被截断");
        assert!(
            json.len() < 600,
            "截断后体积应显著变小，实际 {}",
            json.len()
        );
    }

    #[test]
    fn reasoning_hidden_unless_requested() {
        let batch = MessageBatch {
            entries: vec![msg(
                0,
                EntryKind::Assistant,
                vec![ContentBlock::Reasoning("内部思考".into())],
            )],
            skipped: 0,
        };

        let hidden = build(&meta(), &batch, &ExportOptions::default());
        assert!(!serde_json::to_string(&hidden).unwrap().contains("内部思考"));

        let shown = build(
            &meta(),
            &batch,
            &ExportOptions {
                with_reasoning: true,
                ..Default::default()
            },
        );
        assert!(serde_json::to_string(&shown).unwrap().contains("内部思考"));
    }

    #[test]
    fn unreadable_message_gets_placeholder_instead_of_empty_text() {
        let batch = MessageBatch {
            entries: vec![msg(
                2,
                EntryKind::Synthetic,
                vec![ContentBlock::Other(serde_json::json!({"weird": true}))],
            )],
            skipped: 0,
        };
        let f = build(&meta(), &batch, &ExportOptions::default());
        // 空 text 会让人在 JSON 里一头雾水，给占位符。
        assert!(f.messages[0].text.contains("no readable content"));
    }

    #[test]
    fn skipped_count_is_carried_through() {
        let batch = MessageBatch {
            entries: vec![],
            skipped: 7,
        };
        assert_eq!(
            build(&meta(), &batch, &ExportOptions::default()).skipped_messages,
            7
        );
    }
}
