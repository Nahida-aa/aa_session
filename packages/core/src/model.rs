use serde::{Deserialize, Serialize};

/// 一个项目（一个工作目录）在某个 agent 下的汇总。
///
/// opencode 里对应 `project` 表的一行，其 `worktree` 列就是项目根目录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMeta {
    /// 产生该项目的 agent 应用名（`opencode` 等）。
    pub app: String,
    /// 项目 id（agent 内部 id，仅在本 app 内有意义）。
    pub id: String,
    /// 项目根目录（git 仓库根）。
    ///
    /// **可能是占位值** `"/"`**：opencode 为「无法确定所属仓库」的会话建了一个
    /// 名为 `global` 的兜底项目，worktree 填 `/`（全库仅此一行）。那 24 条会话
    /// 散落在 6 个互不相干的目录里。展示层应把它显示成「未知」而不是根目录。
    ///
    /// 用 [`ProjectMeta::is_unknown_path`] 判断，不要直接和 `"/"` 比较。
    pub path: String,
    /// 版本控制类型（`git` / 空）。
    pub vcs: Option<String>,
    /// 该项目下的**顶层**会话数（与 `sessions` 子命令的默认行为一致）。
    pub session_count: i64,
    /// 该项目下的**子 agent** 会话数。
    pub subagent_count: i64,
    /// 该项目下的**归档**会话数（顶层 + 子会话）。
    pub archived_count: i64,
    /// 项目内最近一次活跃时间（Unix 毫秒）。
    pub time_active: Option<i64>,
}

impl ProjectMeta {
    /// 项目路径是否为「未知」占位符。
    ///
    /// opencode 用字面量 `"/"` 表示「这个会话不属于任何已知仓库」。
    pub fn is_unknown_path(&self) -> bool {
        self.path == UNKNOWN_PROJECT_PATH
    }
}

/// opencode 对「无法确定所属仓库」的项目使用的 worktree 占位值。
pub const UNKNOWN_PROJECT_PATH: &str = "/";

/// 一个会话运行目录的汇总。
///
/// 对应 `session_v2.directory` 的去重结果。存在的理由见
/// [`crate::SessionSource::directories`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryMeta {
    /// 会话运行目录（绝对路径）。
    pub path: String,
    /// 该目录下的会话数（按传入的过滤口径统计）。
    pub session_count: i64,
    /// 该目录下最近一次更新的时间（Unix 毫秒）。
    pub time_updated: i64,
    /// 该目录涉及的**不同项目数**。
    ///
    /// 大于 1 说明有会话跑在了不属于本项目的位置 —— 这是 opencode 的已知
    /// 行为，不是 bug，但对排查有用。
    pub project_count: i64,
}

/// 一个 agent 会话的元信息。
///
/// 没有 `Eq`：`cost` 是 `f64`，浮点不满足 `Eq`。这是刻意接受的代价 —— 为了
/// 在列表里显示真实成本，放弃整个类型的 `Eq` 绑定（`HashSet` / `HashMap` key
/// 用不了；本项目不需要）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// 产生该会话的 agent 应用名。
    pub app: String,
    /// 会话唯一 id。
    pub id: String,
    /// 所属项目 id。
    pub project_id: String,
    /// 所属项目根目录（由 project 表 join 得到；join 不到时回落到会话自身的 directory）。
    pub project_path: String,
    /// 父会话 id。非空表示这是一个派生 / 子会话。
    pub parent_id: Option<String>,
    /// 会话标题。
    pub title: String,
    /// 会话的工作目录。
    pub directory: String,
    /// 使用的 agent 名，如 `build` / `plan`。
    pub agent: Option<String>,
    /// 模型标识。
    pub model: Option<String>,
    /// 创建时间（Unix 毫秒）。
    pub time_created: i64,
    /// 最后更新时间（Unix 毫秒）。
    pub time_updated: i64,
    /// 输入 token 数。
    pub tokens_input: i64,
    /// 输出 token 数。
    pub tokens_output: i64,
    /// 推理（thinking）token 数。
    pub tokens_reasoning: i64,
    /// 累计花费（美元）。
    ///
    /// opencode **只对少数会话维护这个值**（实测 891 条里仅 5 条非零），
    /// 所以 `None` 表示「上游没记录」，与「确实是 0 美元」要区分开。
    pub cost: Option<f64>,
    /// 会话累计改动的文件数。
    pub files_changed: i64,
    /// 归档时间（Unix 毫秒）。`Some` 表示已归档。
    pub archived_at: Option<i64>,
}

impl SessionMeta {
    /// 该会话是否已归档。
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

/// 一条消息在会话中的角色 / 种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// 用户输入。
    User,
    /// 助手回复。
    Assistant,
    /// 助手的思考过程。
    Thought,
    /// 工具调用。
    ToolCall,
    /// 上下文压缩摘要。
    Compaction,
    /// 框架自动生成的伪消息。
    Synthetic,
    /// 系统消息。
    System,
}

impl EntryKind {
    /// 该类型是否为人类可读内容（导出时决定是否折叠 tool 输出）。
    pub fn is_readable(self) -> bool {
        matches!(
            self,
            EntryKind::User
                | EntryKind::Assistant
                | EntryKind::Thought
                | EntryKind::Compaction
                | EntryKind::System
        )
    }
}

/// 消息内容的一个分块。
///
/// 分层思想借鉴 ACP 的 `ContentBlock`，但这里是**持久化/导出模型**而非流式
/// 协议模型 —— 不需要 `chunk` 增量语义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// 纯文本。
    Text(String),

    /// 助手的思考内容。
    Reasoning(String),

    /// 工具调用。
    ToolCall {
        /// 工具调用 id。
        id: String,
        /// 工具名，如 `read` / `bash`。
        name: String,
        /// 状态，如 `completed` / `error` / `running`。
        status: Option<String>,
        /// 原始入参 JSON。
        input: Option<serde_json::Value>,
        /// 原始输出 JSON。
        output: Option<serde_json::Value>,
    },

    /// 文件改动。
    Diff {
        /// 变更文件路径。
        path: String,
        /// unified diff 文本。
        patch: String,
    },

    /// 上游新增了我们还不认识的块类型，原样保留 JSON 以免丢信息。
    Other(serde_json::Value),
}

/// 会话中的一条消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageEntry {
    /// 消息 id。
    pub id: String,
    /// 会话内的顺序号。
    pub seq: i64,
    /// 角色 / 种类。
    pub kind: EntryKind,
    /// 内容分块。
    pub content: Vec<ContentBlock>,
    /// 创建时间（Unix 毫秒）。
    pub time_created: i64,
}

/// 一个会话的消息读取结果。
///
/// 单条消息损坏不该让整个会话作废，所以 source 层跳过坏行并在这里报数。
#[derive(Debug, Clone)]
pub struct MessageBatch {
    /// 成功解析的消息，按 `seq` 升序。
    pub entries: Vec<MessageEntry>,
    /// 因 JSON 损坏被跳过的消息数。
    pub skipped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> SessionMeta {
        SessionMeta {
            app: "opencode".into(),
            id: "s".into(),
            project_id: "p".into(),
            project_path: "/tmp".into(),
            parent_id: None,
            title: String::new(),
            directory: "/tmp".into(),
            agent: None,
            model: None,
            time_created: 0,
            time_updated: 0,
            tokens_input: 0,
            tokens_output: 0,
            tokens_reasoning: 0,
            cost: None,
            files_changed: 0,
            archived_at: None,
        }
    }

    #[test]
    fn is_archived_reflects_archived_at() {
        let mut m = meta();
        assert!(!m.is_archived());
        m.archived_at = Some(1);
        assert!(m.is_archived());
    }

    #[test]
    fn cost_round_trips_as_none_not_zero() {
        // 上游用 0.0 表示「没记录」，与「真的花了 0 美元」必须能区分开。
        let m = meta();
        assert_eq!(m.cost, None);
        let mut m2 = meta();
        m2.cost = Some(0.0);
        assert_eq!(m2.cost, Some(0.0));
    }
}

#[cfg(test)]
mod project_meta_tests {
    use super::*;

    fn meta(path: &str) -> ProjectMeta {
        ProjectMeta {
            app: "opencode".into(),
            id: "p".into(),
            path: path.to_string(),
            vcs: None,
            session_count: 0,
            subagent_count: 0,
            archived_count: 0,
            time_active: None,
        }
    }

    #[test]
    fn slash_is_the_unknown_sentinel_not_a_real_root() {
        assert!(meta("/").is_unknown_path());
    }

    #[test]
    fn real_paths_are_not_unknown() {
        assert!(!meta("/home/aa/repos").is_unknown_path());
        assert!(!meta("").is_unknown_path());
        // 双斜杠开头的变体不应被误判成未知
        assert!(!meta("//").is_unknown_path());
    }
}
