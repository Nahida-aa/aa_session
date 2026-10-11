use crate::Result;
use crate::model::{DirectoryMeta, MessageBatch, MessageEntry, ProjectMeta, SessionMeta};

/// 子 agent 会话的纳入方式。
///
/// opencode 里 891 个会话有 **652 个是子 agent 会话**（`parent_id` 非空），
/// 几乎全是主 agent 派出的 `explore` 探查任务。它们标题雷同、目录相同、各自
/// 消耗 token，默认混在列表里会把真正想找的顶层会话淹没 —— 所以默认只显示
/// 顶层会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Subagents {
    /// 只显示顶层会话（默认）。
    #[default]
    Exclude,
    /// 顶层与子会话都显示。
    Include,
    /// 只显示子会话。
    Only,
}

impl Subagents {
    /// 该模式对应的 SQL 片段（无绑定参数）。
    pub fn sql_filter(self) -> &'static str {
        match self {
            Subagents::Exclude => " AND s.parent_id IS NULL",
            Subagents::Include => "",
            Subagents::Only => " AND s.parent_id IS NOT NULL",
        }
    }
}

/// 归档会话的纳入方式。
///
/// 归档是用户主动收起一个会话，正常浏览时不该出现。但归档会话里可能有想找回
/// 的东西，所以留一个显式开关而不是直接删掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Archived {
    /// 排除归档会话（默认）。
    #[default]
    Exclude,
    /// 只显示归档会话。
    Only,
    /// 归档与未归档都显示。
    Include,
}

impl Archived {
    /// 该模式对应的 SQL 片段（无绑定参数）。
    pub fn sql_filter(self) -> &'static str {
        match self {
            Archived::Exclude => " AND s.time_archived IS NULL",
            Archived::Only => " AND s.time_archived IS NOT NULL",
            Archived::Include => "",
        }
    }
}

/// 项目寻址方式。
///
/// # 为什么用路径而不是项目 id
///
/// opencode 的 `project.id` 是 40 字符哈希（实测不等于路径，也不是路径的 sha1），
/// 没人能记住；而路径本来就是用户脑子里的项目标识。对外**只接受绝对路径**。
///
/// 但 `project.worktree` 存在重复：实测 46 个项目行只有 45 个不同 worktree，
/// `/home/aa/repos/env_ls/Nahida-aa.github.io` 有**两个独立 id**（7 + 11 = 18 条
/// 会话）。所以按路径查询时必须把所有同路径的 id 聚合起来。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectSelector {
    /// 精确匹配 `project.worktree` 全等。
    ///
    /// 聚合同路径的所有 project id，因此**永不歧义**。
    Exact {
        /// 用户的绝对路径输入。
        path: String,
    },
    /// 路径子串模糊匹配。
    ///
    /// 多命中是预期行为（这正是粗筛的目的），所以 `Include` 表示全部返回。
    Like {
        /// 子串。
        pattern: String,
        /// 命中项目的 worktree 全集（由上层解析得到）。
        paths: Vec<String>,
    },
    /// 不按项目过滤。
    #[default]
    All,
}

/// 空壳会话（一条消息都没有）的纳入方式。
///
/// opencode 会为「打开过但从未真正使用」的会话建行：0 消息、0 token，标题是
/// `New session - <ISO 时间戳>`。opencode 自己的 UI 不显示它们，我们**也默认
/// 不显示**——否则列表里混着 21% 点不开、导不出、也没法 fork 的空行。
///
/// 实测顶层未归档会话 222 条里有 **46 条**是空壳。没有任何字段能区分它们
/// （`time_viewed` / `time_idle` / `idle_outcome` 在空壳和真会话上都是 NULL），
/// 唯一可靠信号是**消息数为 0**，所以只能用 `NOT EXISTS`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmptySessions {
    /// 排除空壳会话（默认）。
    #[default]
    Exclude,
    /// 只显示空壳会话。
    Only,
    /// 空壳与正常会话都显示。
    Include,
}

impl EmptySessions {
    /// 该模式对应的 SQL 片段（无绑定参数）。
    ///
    /// 空壳判定用 `session_message`（v2 schema）—— v2 里一条会话没有
    /// 消息意味着它是占位符。
    pub fn sql_filter(self) -> &'static str {
        match self {
            EmptySessions::Exclude => " AND EXISTS (SELECT 1 FROM session_message m \
                                       WHERE m.session_id = s.id)",
            EmptySessions::Only => " AND NOT EXISTS (SELECT 1 FROM session_message m \
                                   WHERE m.session_id = s.id)",
            EmptySessions::Include => "",
        }
    }

    /// 同 [`Self::sql_filter`]，但用 v1 的 `message` 表判定空壳。
    ///
    /// 为什么需要两个版本：`session`（v1）的会话，消息在 `message` 表；
    /// `session_v2`（v2）的会话，消息在 `session_message` 表。同一库两套
    /// schema 并存，空壳判定必须跟着各自的消息表走。
    pub fn sql_filter_v1(self) -> &'static str {
        match self {
            EmptySessions::Exclude => " AND EXISTS (SELECT 1 FROM message m \
                                       WHERE m.session_id = s.id)",
            EmptySessions::Only => " AND NOT EXISTS (SELECT 1 FROM message m \
                                   WHERE m.session_id = s.id)",
            EmptySessions::Include => "",
        }
    }
}

/// 会话列表的过滤条件。
///
/// 所有条件以 `AND` 叠加，空值表示「不限制」。
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// 按项目寻址。见 [`ProjectSelector`]。
    pub project: ProjectSelector,
    /// 只列运行目录包含该子串的会话（`session_v2.directory`，**不是**项目目录）。
    pub directory: Option<String>,
    /// 只列这些 agent 名。
    pub agents: Vec<String>,
    /// 只列标题包含该子串的会话。
    pub title_contains: Option<String>,
    /// 只列 `time_updated >=` 该 Unix 毫秒值的会话。
    pub updated_after: Option<i64>,
    /// 子 agent 会话的纳入方式。
    pub subagents: Subagents,
    /// 归档会话的纳入方式。
    pub archived: Archived,
    /// 空壳会话（0 消息）的纳入方式。
    pub empty: EmptySessions,
    /// 最多返回多少条。
    pub limit: usize,
}

/// 会话列表的排序键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// 按最后更新时间倒序（默认）—— 最常用的「最近在聊什么」。
    #[default]
    UpdatedDesc,
    /// 按创建时间倒序。
    CreatedDesc,
    /// 按标题排序。
    Title,
    /// 按总 token 数倒序 —— 找最贵的会话。
    Tokens,
}

/// 一个本地 agent 会话数据源。
///
/// **v1 只有 opencode 一个实现**。trait 按可扩展设计：未来的 Claude Code
/// （`~/.claude/projects/**/*.jsonl`）、Codex（`~/.codex/sessions/**/*.jsonl`）
/// 只需再写一个实现，各自直读自己的存储 —— 不需要统一的中间索引。
///
/// # 只读契约
///
/// 本 trait 的所有方法**只读**。fork / edit 由上层通过 fork-first 流程
/// （复制出副本再改）实现，**不写回原始数据源** —— 原始库可能正被运行中的
/// agent 进程持有，写入会与其内存状态冲突。
pub trait SessionSource {
    /// 数据源标识：产生这些会话的 agent 应用名，如 `"opencode"`。
    ///
    /// 它同时表达「来源」和「目标」。
    fn app(&self) -> &'static str;

    /// 列出所有项目及其会话数，按最近活跃时间倒序。
    fn projects(&self) -> Result<Vec<ProjectMeta>>;

    /// 模糊查找项目路径：返回所有 `worktree` 含该子串的项目路径（已去重）。
    ///
    /// 放在 trait 里是因为「路径寻址」是与具体 app 无关的概念：Claude Code
    /// 之类的实现也该提供同样语义。
    fn project_paths_like(&self, pattern: &str) -> Result<Vec<String>>;

    /// 列出所有**会话运行目录**及其会话数。
    ///
    /// 与 [`SessionSource::projects`] 的区别是必要的：会话的运行目录和它归属的
    /// 项目**不是一回事**。实测 opencode 有两类不一致：
    ///
    /// - 会话跑在项目仓库根的**子目录**里（14 条）；
    /// - 会话跑在**另一个仓库**里却归到原项目（aaBot 有 5 条跑在 AAgent 目录）；
    /// - `global` 兜底项目的 24 条会话散落在 6 个互不相干的目录。
    ///
    /// 所以「这个 agent 在哪工作过」只能靠这个方法回答。
    fn directories(&self) -> Result<Vec<DirectoryMeta>>;

    /// 按过滤条件列出会话。
    fn sessions(&self, filter: &SessionFilter, sort: SortKey) -> Result<Vec<SessionMeta>>;

    /// 读取单个会话的元信息；不存在返回 `Ok(None)`。
    fn session(&self, session_id: &str) -> Result<Option<SessionMeta>>;

    /// 读取单个会话的全部消息，按 `seq` 升序。
    fn messages(&self, session_id: &str) -> Result<MessageBatch>;

    /// 把单个消息读出来（供将来 fork 定位边界用）。
    fn message(&self, session_id: &str, seq: i64) -> Result<Option<MessageEntry>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagents_default_to_excluded() {
        // 默认值必须是 Exclude —— 891 个会话里 652 个是子 agent，
        // 混在一起会淹没真正想找的顶层会话。
        assert_eq!(Subagents::default(), Subagents::Exclude);
        assert_eq!(SessionFilter::default().subagents, Subagents::Exclude);
    }

    #[test]
    fn archived_defaults_to_excluded() {
        assert_eq!(Archived::default(), Archived::Exclude);
        assert_eq!(SessionFilter::default().archived, Archived::Exclude);
    }

    #[test]
    fn archived_modes_emit_distinct_predicates() {
        assert_eq!(
            Archived::Exclude.sql_filter(),
            " AND s.time_archived IS NULL"
        );
        assert_eq!(
            Archived::Only.sql_filter(),
            " AND s.time_archived IS NOT NULL"
        );
        assert_eq!(Archived::Include.sql_filter(), "");
    }

    #[test]
    fn exclude_filters_on_null_parent() {
        assert_eq!(Subagents::Exclude.sql_filter(), " AND s.parent_id IS NULL");
    }

    #[test]
    fn include_adds_no_predicate() {
        // Include 不能加任何条件，否则会误过滤。
        assert_eq!(Subagents::Include.sql_filter(), "");
    }

    #[test]
    fn only_filters_on_non_null_parent() {
        assert_eq!(Subagents::Only.sql_filter(), " AND s.parent_id IS NOT NULL");
    }

    #[test]
    fn each_mode_emits_a_distinct_predicate() {
        let mut all = vec![
            Subagents::Exclude.sql_filter(),
            Subagents::Include.sql_filter(),
            Subagents::Only.sql_filter(),
        ];
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 3, "三种模式必须产生互不相同的 SQL 片段");
    }
}

#[cfg(test)]
mod project_selector_tests {
    use super::*;

    #[test]
    fn default_selector_is_all() {
        assert_eq!(SessionFilter::default().project, ProjectSelector::All);
    }

    #[test]
    fn exact_carries_the_user_path_verbatim() {
        // 规范化在 CLI 层做（去尾部斜杠、展开 ~），core 不再改写，
        // 免得同一处逻辑散在两层。
        let sel = ProjectSelector::Exact {
            path: "/a/b".into(),
        };
        match sel {
            ProjectSelector::Exact { path } => assert_eq!(path, "/a/b"),
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn like_carries_pattern_and_resolved_paths() {
        let sel = ProjectSelector::Like {
            pattern: "Nahida".into(),
            paths: vec!["/a/Nahida".into(), "/b/Nahida-x".into()],
        };
        match sel {
            ProjectSelector::Like { pattern, paths } => {
                assert_eq!(pattern, "Nahida");
                // 多命中是模糊匹配的预期行为，不该报错。
                assert_eq!(paths.len(), 2);
            }
            other => panic!("expected Like, got {other:?}"),
        }
    }
}
