//! 旧 schema（v1）`session` / `message` / `part` 三表的适配层。
//!
//! # 为什么需要
//!
//! opencode 迁移到 `session_v2` 后**不会删除**旧行。实测同一库里
//! `session` 有 894 行、`session_v2` 有 891 行——v2-only 4 条，v1-only 7 条
//! （6 条在 v2 里也有对应，**真正只存在于 v1 的有 6 条**，其中 3 条有内容）。
//!
//! 这些是早期建的会话，opencode 的 UI **两表都读**，所以它显示 6 条而只读
//! v2 的工具显示 3 条——这是用户报的「对不上」的真因。
//!
//! # 两套 schema 的实际差异（已实测）
//!
//! |          | v1 `message` | v2 `session_message` |
//!|----------|--------------|---------------------|
//! | 角色     | 顶层字段 `data.role` | 独立列 `type` |
//! | 正文     | 不在 message 里，在 `part.data`（JSON） | 同一行 `data` 列 |
//! | 顺序     | 无 `seq` 列，靠 `time_created` / `id` | 有 `seq` 列 |
//! | 分块     | `part` 表：text/tool/reasoning/patch/compaction/step-* | `data.content[]` 数组 |
//!
//! 所以这里把 v1 的 `message.role` 映射成 v2 的 `type`，把 `part.data` 拼回
//! 一个 `Vec<ContentBlock>`，下游（`export.rs` 的 `build`、CLI 展示）**不
//! 需要知道这条消息来自哪个 schema**。

use aa_session_core::ContentBlock;



/// 把 v1 的 `message.role` 映射到 v2 `session_message.type` 的取值域。
///
/// 未知值降级为 `"synthetic"`，与 [`aa_session_core::entry_kind_from_type`]
/// 对未知类型的兜底行为一致。
pub fn v1_role_to_v2_type(role: &str) -> &'static str {
    match role {
        "user" => "user",
        "assistant" => "assistant",
        "tool" => "tool",
        "compaction" => "compaction",
        "system" => "system",
        "thought" => "thought",
        _ => "synthetic",
    }
}

/// 把一条 v1 `message` 及其 `part` 拼成 [`aa_session_core::MessageEntry`]。
///
/// 顺序：v1 没有 `seq`，用调用方给定的 `seq`（按 `time_created` 升序编号）。
pub fn build_v1_message(
    seq: i64,
    message_id: &str,
    time_created: i64,
    role: &str,
    parts: &[crate::v1_structs::PartBlock],
) -> aa_session_core::MessageEntry {
    use aa_session_core::MessageEntry;

    let kind = crate::blocks::entry_kind_from_type(v1_role_to_v2_type(role));
    let content = parts.iter().filter_map(part_to_block).collect();

    MessageEntry {
        id: message_id.to_string(),
        seq,
        kind,
        content,
        time_created,
    }
}

/// 单个 v1 `part` 转成 [`ContentBlock`]。
fn part_to_block(part: &crate::v1_structs::PartBlock) -> Option<ContentBlock> {
    use aa_session_core::ContentBlock as B;
    use crate::v1_structs::PartBlock as P;

    match part {
        P::Text { text } => Some(B::Text(text.clone())),

        P::Reasoning { text } => {
            if text.is_empty() {
                None
            } else {
                Some(B::Reasoning(text.clone()))
            }
        }

        P::Tool { tool, state } => {
            // 输出优先级：content 里的 text 拼接 → 失败时退回 error。
            let content = state.as_ref().and_then(|s| s.content.as_ref());
            let output = match content {
                Some(items) => {
                    let text: Vec<&str> = items
                        .iter()
                        .map(|c| c.text.as_str())
                        .filter(|t| !t.is_empty())
                        .collect();
                    if text.is_empty() {
                        state.as_ref().and_then(|s| s.error.clone())
                    } else {
                        Some(text.join("\n"))
                    }
                }
                None => None,
            };

            Some(B::ToolCall {
                id: tool.clone(),
                name: tool.clone(),
                status: state.as_ref().and_then(|s| s.status.clone()),
                input: state.as_ref().and_then(|s| s.input.clone()),
                output: output.filter(|s| !s.is_empty()).map(serde_json::Value::String),
            })
        }

        P::Patch { patch } => patch
            .as_ref()
            .and_then(|p| p.files.first())
            .map(|f| B::Diff {
                path: f.file.clone(),
                patch: f.patch.clone(),
            }),

        P::Compaction { summary } => summary
            .clone()
            .filter(|s| !s.is_empty())
            .map(B::Text),

        P::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1_structs::PartBlock;

    #[test]
    fn role_maps_match_v2_defaults() {
        // 未知角色必须落到 "synthetic"（下游兜底为 EntryKind::Synthetic），
        // 绝不能落到 "system" 或 "user"，否则过滤逻辑会错位。
        assert_eq!(v1_role_to_v2_type("user"), "user");
        assert_eq!(v1_role_to_v2_type("assistant"), "assistant");
        assert_eq!(v1_role_to_v2_type("garbage"), "synthetic");
    }

    #[test]
    fn text_part_becomes_text_block() {
        let parts = vec![PartBlock::Text {
            text: "你好".into(),
        }];
        let m = build_v1_message(0, "msg_1", 0, "user", &parts);
        assert_eq!(m.content, vec![ContentBlock::Text("你好".into())]);
    }

    #[test]
    fn tool_part_captures_input_and_output() {
        let parts = vec![PartBlock::Tool {
            tool: "read".into(),
            state: Some(crate::v1_structs::V1ToolState {
                status: Some("completed".into()),
                input: Some(serde_json::json!({"path":"a.rs"})),
                content: Some(vec![crate::v1_structs::V1TextContent {
                    text: "fn main() {}".into(),
                }]),
                error: None,
            }),
        }];
        let m = build_v1_message(1, "msg_2", 0, "assistant", &parts);
        if let Some(ContentBlock::ToolCall {
            name,
            input,
            output,
            ..
        }) = m.content.first()
        {
            assert_eq!(name, "read");
            assert!(input.is_some());
            assert_eq!(output.as_ref().unwrap(), "fn main() {}");
        } else {
            panic!("expected ToolCall, got {:?}", m.content);
        }
    }

    #[test]
    fn unknown_part_type_is_dropped_not_crashing() {
        // step-start / step-finder 这类纯结构块没有正文，应当被跳过。
        let parts = vec![PartBlock::Other];
        let m = build_v1_message(2, "msg_3", 0, "assistant", &parts);
        assert!(m.content.is_empty());
    }

    #[test]
    fn patch_part_becomes_diff_block() {
        let parts = vec![PartBlock::Patch {
            patch: Some(crate::v1_structs::V1Patch {
                files: vec![crate::v1_structs::V1PatchFile {
                    file: "src/main.rs".into(),
                    patch: "@@ x @@".into(),
                }],
            }),
        }];
        let m = build_v1_message(3, "msg_4", 0, "assistant", &parts);
        assert_eq!(
            m.content,
            vec![ContentBlock::Diff {
                path: "src/main.rs".into(),
                patch: "@@ x @@".into(),
            }]
        );
    }
}
