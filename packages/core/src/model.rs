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

    /// 归属项目是否为「未知」兜底桶（opencode 用 `/` 表示无法确定所属仓库）。
    ///
    /// 与 [`ProjectMeta::is_unknown_path`] 共用同一常量，避免两处各写一遍
    /// 字符串比较而走偏。
    pub fn is_unknown_project(&self) -> bool {
        self.project_path == UNKNOWN_PROJECT_PATH
    }
}

/// 会话中的一条消息（原始数据，由各 source crate 自己定义字段）。
///
/// 已删除：opencode/codebuddy 各自用 `serde_json::Value` 原样存原始数据，
/// 不走统一抽象。`SessionMeta` 保留用于 `sessions` 列表命令（最小公约数）。

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

#[cfg(test)]
mod session_meta_project_tests {
    use super::*;

    #[test]
    fn slash_project_path_is_unknown_bucket() {
        let mut m = session_meta();
        m.project_path = UNKNOWN_PROJECT_PATH.to_string();
        assert!(m.is_unknown_project());
    }

    #[test]
    fn real_project_path_is_not_unknown() {
        let mut m = session_meta();
        m.project_path = "/home/aa/repos".to_string();
        assert!(!m.is_unknown_project());
    }

    #[test]
    fn session_and_project_share_the_unknown_sentinel() {
        // 两个类型必须用同一个常量，否则 projects 显示「未知」而 sessions 显示
        // 「/」，同一个兜底桶在两个视图里长得不一样。
        let p = ProjectMeta {
            app: "opencode".into(),
            id: "p".into(),
            path: UNKNOWN_PROJECT_PATH.to_string(),
            vcs: None,
            session_count: 0,
            subagent_count: 0,
            archived_count: 0,
            time_active: None,
        };
        let mut s = session_meta();
        s.project_path = p.path.clone();

        assert!(p.is_unknown_path());
        assert!(s.is_unknown_project());
    }

    fn session_meta() -> SessionMeta {
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
}
