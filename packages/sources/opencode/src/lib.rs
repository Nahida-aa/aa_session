//! opencode 会话数据源（只读，直读 SQLite）。
//!
//! 目标库：`~/.local/share/opencode/opencode.db`（SQLite/WAL，约 6 GB）。
//!
//! 读 `session_v2` / `session_message` / `project`（v2 schema）。v1 的
//! `session` / `message` / `part` 三表**不读** —— 同一库里两套都在，但 v2
//! 才是当前 schema。
//!
//! # 为什么直读而不是走 HTTP API
//!
//! opencode 提供 `opencode serve`（`GET /session` 等）。不用它的原因：
//!
//! 1. **它要求 opencode 进程在跑**，而我们的核心场景恰恰是关着 opencode
//!    时翻查历史会话；
//! 2. 要取全部消息正文得逐个 session 发 HTTP 请求（当前 891 个会话、
//!    20 万条消息），而本地一次 SQL 查询即可；
//! 3. 写路径本来就不该碰——那正是与运行中进程抢状态。
//!
//! # 只读打开
//!
//! 以 SQLite `mode=ro` URI 打开，**永不写入**。

pub mod backup;
pub mod blocks;
pub mod export;
pub mod rename;

use std::path::{Path, PathBuf};

use aa_session_core::{
    DirectoryMeta, Error, MessageBatch, MessageEntry, ProjectMeta, ProjectSelector, Result,
    SessionFilter, SessionMeta, SessionSource, SortKey,
};
use rusqlite::{Connection, OpenFlags, Row, params};

use blocks::{entry_kind_from_type, parse_blocks};

/// 默认的 opencode 数据目录（相对 `$HOME`）。
pub const DEFAULT_DATA_DIR: &str = ".local/share/opencode";

/// 数据库文件名。
pub const DB_FILE_NAME: &str = "opencode.db";

/// 本数据源的应用名。
pub const APP_NAME: &str = "opencode";

/// 会话列。与 [`SESSION_FROM_PROJECT`] 的列序保持一致。
const SESSION_COLUMNS: &str = "s.id, s.project_id, COALESCE(p.worktree, ''), s.parent_id, \
                              s.title, s.directory, s.agent, s.model, \
                              s.time_created, s.time_updated, s.tokens_input, s.tokens_output, \
                              s.tokens_reasoning, s.cost, COALESCE(s.summary_files, 0), \
                              s.time_archived";

/// 同上，但从 project 表 join 出 `project_path`。
const SESSION_FROM_PROJECT: &str = "FROM session_v2 s \
                                   LEFT JOIN project p ON p.id = s.project_id";

/// opencode 的只读数据源。
pub struct OpencodeSource {
    conn: Connection,
    /// 打开时解析出的绝对路径，仅用于错误信息。
    path: PathBuf,
}

impl OpencodeSource {
    /// 从数据目录（如 `~/.local/share/opencode`）打开。
    pub fn open_dir(dir: &Path) -> Result<Self> {
        Self::open(&dir.join(DB_FILE_NAME))
    }

    /// 从 `$HOME` 展开后的默认位置打开。
    pub fn open_default() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::InvalidArgument("HOME is not set".into()))?;
        Self::open_dir(&home.join(DEFAULT_DATA_DIR))
    }

    /// 以只读模式打开指定数据库文件。
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_impl(path, false)
    }

    /// 以**读写**模式打开指定数据库文件。
    ///
    /// 只给写路径（改名）用；其余一律走 [`Self::open`]。读写连接与只读连接是
    /// 两个不同的构造入口，这样「不小心写了库」在类型层面就不可能发生。
    pub fn open_writable(path: &Path) -> Result<Self> {
        Self::open_impl(path, true)
    }

    fn open_impl(path: &Path, writable: bool) -> Result<Self> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !path.exists() {
            return Err(Error::Open {
                path: path.display().to_string(),
                cause: "file does not exist".into(),
            });
        }

        // `mode=ro` 从 SQLite 层保证只读，比打开后再靠代码自律可靠。
        // 写路径用普通 RW 连接，并交给 SQLite 自己管 WAL。
        let (uri, flags) = if writable {
            (
                format!("file:{}", path.display()),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
            )
        } else {
            (
                format!("file:{}?mode=ro", path.display()),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
        };
        let conn = Connection::open_with_flags(&uri, flags).map_err(|e| Error::Open {
            path: path.display().to_string(),
            cause: e.to_string(),
        })?;

        // 6 GB 的库，缺索引的查询会全表扫。这里只影响速度，不影响正确性。
        let _ = conn.pragma_update(None, "mmap_size", 1_073_741_824); // 1 GiB
        let _ = conn.pragma_update(None, "cache_size", -65_536); // 64 MiB

        Ok(Self { conn, path })
    }

    /// 数据库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 只读连接。改名预检用（全是 `SELECT`）。
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// 可写连接。**只有改名路径能用**。
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// 按 `worktree` 全等找出 project id；同路径的多个 id 全部返回。
    ///
    /// 返回空 `Vec` 表示该路径下没有项目。改名预检用它确认「源存在、
    /// 目标未被占用」。
    pub fn project_ids_for_path(&self, path: &str) -> Result<Vec<String>> {
        self.collect_rows(
            "SELECT id FROM project WHERE worktree = ?",
            &[&path],
            |row| row.get(0).map_err(query_err),
        )
    }

    /// 执行查询并把行收集成 `Vec`。
    fn collect_rows<T>(
        &self,
        sql: &str,
        binds: &[&dyn rusqlite::ToSql],
        map_row: impl Fn(&Row<'_>) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare(sql).map_err(query_err)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(binds.iter()))
            .map_err(query_err)?;

        let mut out = Vec::new();
        // `Rows` 只实现了 `next()`，不是 `Iterator` —— 必须手写循环。
        while let Some(row) = rows.next().map_err(query_err)? {
            out.push(map_row(row)?);
        }
        Ok(out)
    }
}

/// 把 `rusqlite` 错误转成领域错误。
fn query_err(e: rusqlite::Error) -> Error {
    Error::Query(e.to_string())
}

/// 把一行会话记录读成 [`SessionMeta`]。
///
/// 列序必须与 [`SESSION_COLUMNS`] 一致。
fn session_from_row(row: &Row<'_>) -> Result<SessionMeta> {
    let directory: String = row.get(5).map_err(query_err)?;
    let project_path: String = row.get(2).map_err(query_err)?;

    Ok(SessionMeta {
        app: APP_NAME.to_string(),
        id: row.get(0).map_err(query_err)?,
        project_id: row.get(1).map_err(query_err)?,
        // join 不到 project 行时（理论上不该发生）回落到会话自身的目录，
        // 总比空串有用。
        project_path: if project_path.is_empty() {
            directory.clone()
        } else {
            project_path
        },
        parent_id: row.get(3).map_err(query_err)?,
        // title 实际允许 NULL（与列声明的 NOT NULL 有出入），兜底成空串。
        title: row
            .get::<_, Option<String>>(4)
            .map_err(query_err)?
            .unwrap_or_default(),
        directory,
        agent: row.get(6).map_err(query_err)?,
        model: row.get(7).map_err(query_err)?,
        time_created: row.get(8).map_err(query_err)?,
        time_updated: row.get(9).map_err(query_err)?,
        tokens_input: row.get(10).map_err(query_err)?,
        tokens_output: row.get(11).map_err(query_err)?,
        tokens_reasoning: row.get(12).map_err(query_err)?,
        // opencode 对绝大多数会话写的是 0.0，真正有意义的只有非零值；
        // 把 0 归一成 None，表格里就不用满屏 `$0.00`。
        cost: match row.get::<_, f64>(13).map_err(query_err)? {
            c if c > 0.0 => Some(c),
            _ => None,
        },
        files_changed: row.get(14).map_err(query_err)?,
        archived_at: row.get(15).map_err(query_err)?,
    })
}

/// 转义 LIKE 的通配符，让用户输入被当作字面量。
fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 按过滤条件拼 `WHERE` 子句与绑定值。
fn build_where(filter: &SessionFilter) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut sql = String::from(" WHERE 1 = 1");
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    fn push_in(
        sql: &mut String,
        binds: &mut Vec<Box<dyn rusqlite::ToSql>>,
        col: &str,
        vals: &[String],
    ) {
        if vals.is_empty() {
            return;
        }
        let holes = vals.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND {col} IN ({holes})"));
        for v in vals {
            binds.push(Box::new(v.clone()));
        }
    }

    // 子 agent 会话默认排除 —— 见 `Subagents` 的文档。
    sql.push_str(filter.subagents.sql_filter());
    // 归档会话默认排除 —— 见 `Archived` 的文档。
    sql.push_str(filter.archived.sql_filter());

    // 项目过滤走 project 表的 worktree，不能直接拿 s.project_id —— 因为
    // 同路径可能有多个 project id（实测 Nahida 那条路径就有 2 个）。
    match &filter.project {
        ProjectSelector::All => {}
        ProjectSelector::Exact { path } => {
            sql.push_str(" AND s.project_id IN (SELECT id FROM project WHERE worktree = ?)");
            binds.push(Box::new(path.clone()));
        }
        ProjectSelector::Like { paths, .. } => {
            // paths 是 **worktree** 不是 id，必须经 project 表再转一层；
            // 直接拿去 IN project_id 会一条都匹配不到。
            let holes = paths.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            sql.push_str(&format!(
                " AND s.project_id IN (SELECT id FROM project WHERE worktree IN ({holes}))"
            ));
            for p in paths {
                binds.push(Box::new(p.clone()));
            }
        }
    }

    push_in(&mut sql, &mut binds, "s.agent", &filter.agents);

    if let Some(dir) = &filter.directory {
        // LIKE 而非 GLOB：路径里的 `*` / `?` 在 GLOB 中是通配符，
        // 而用户想的是子串匹配。
        sql.push_str(" AND s.directory LIKE ? ESCAPE '\\'");
        binds.push(Box::new(format!("%{}%", escape_like(dir))));
    }

    if let Some(t) = &filter.title_contains {
        sql.push_str(" AND s.title LIKE ? ESCAPE '\\' COLLATE NOCASE");
        binds.push(Box::new(format!("%{}%", escape_like(t))));
    }

    if let Some(after) = filter.updated_after {
        sql.push_str(" AND s.time_updated >= ?");
        binds.push(Box::new(after));
    }

    (sql, binds)
}

/// 排序子句。都以 `id` 收尾保证**排序稳定**，否则同一批里顺序随机。
fn order_by(sort: SortKey) -> &'static str {
    match sort {
        SortKey::UpdatedDesc => "s.time_updated DESC, s.id ASC",
        SortKey::CreatedDesc => "s.time_created DESC, s.id ASC",
        SortKey::Title => "s.title COLLATE NOCASE ASC, s.id ASC",
        SortKey::Tokens => "(s.tokens_input + s.tokens_output) DESC, s.id ASC",
    }
}

/// 把一行 `session_message` 读成 [`MessageEntry`]。
fn message_from_row(row: &Row<'_>) -> Result<MessageEntry> {
    let id: String = row.get(0).map_err(query_err)?;
    let type_str: String = row.get(1).map_err(query_err)?;
    let seq: i64 = row.get(2).map_err(query_err)?;
    let time_created: i64 = row.get(3).map_err(query_err)?;
    let data: String = row.get(4).map_err(query_err)?;

    let kind = entry_kind_from_type(&type_str);

    // 坏 JSON 报出 message id 和 type，便于定位。
    let content = parse_blocks(kind, &data).map_err(|cause| Error::MalformedJson {
        location: format!("message:{id} (type={type_str})"),
        cause,
    })?;

    Ok(MessageEntry {
        id,
        seq,
        kind,
        content,
        time_created,
    })
}

impl SessionSource for OpencodeSource {
    fn app(&self) -> &'static str {
        APP_NAME
    }

    /// 列出所有项目及其会话数。
    ///
    /// `session_count` 只数**顶层会话**，与 [`SessionSource::sessions`] 的默认
    /// 行为保持一致 —— 否则 `projects` 显示 274 而 `sessions` 只列出 100 多条，
    /// 用户会以为哪里坏了。子会话数另放在 `subagent_count`。
    ///
    /// 用 `LEFT JOIN` 而不是 `JOIN`：没有会话的项目也该出现（否则用户会
    /// 以为项目不存在）。
    fn projects(&self) -> Result<Vec<ProjectMeta>> {
        let sql = "SELECT p.id, COALESCE(p.worktree, ''), p.vcs,
                          (SELECT count(*) FROM session_v2 s
                            WHERE s.project_id = p.id AND s.parent_id IS NULL
                              AND s.time_archived IS NULL),
                          (SELECT count(*) FROM session_v2 s
                            WHERE s.project_id = p.id AND s.parent_id IS NOT NULL
                              AND s.time_archived IS NULL),
                          (SELECT count(*) FROM session_v2 s
                            WHERE s.project_id = p.id AND s.time_archived IS NOT NULL),
                          p.time_active
                   FROM project p
                   ORDER BY p.time_active DESC, p.id ASC";

        self.collect_rows(sql, &[], |row| {
            Ok(ProjectMeta {
                app: APP_NAME.to_string(),
                id: row.get(0).map_err(query_err)?,
                path: row.get(1).map_err(query_err)?,
                vcs: row.get(2).map_err(query_err)?,
                session_count: row.get(3).map_err(query_err)?,
                subagent_count: row.get(4).map_err(query_err)?,
                archived_count: row.get(5).map_err(query_err)?,
                time_active: row.get(6).map_err(query_err)?,
            })
        })
    }

    /// 列出所有会话运行目录（`session_v2.directory` 去重）。
    ///
    /// `project_count` 用 `COUNT(DISTINCT project_id)` 而不是布尔聚合 ——
    /// 一个目录完全可能同时包含本项目的会话和「跑错位置」的会话。
    /// 统计口径对齐 `sessions` 的默认行为（排除子 agent 与归档），否则
    /// `directories` 和 `sessions` 的数字会对不上。
    fn directories(&self) -> Result<Vec<DirectoryMeta>> {
        let sql = "SELECT s.directory,
                          count(*) AS n,
                          max(s.time_updated) AS updated,
                          count(DISTINCT s.project_id) AS projects
                   FROM session_v2 s
                   WHERE s.parent_id IS NULL AND s.time_archived IS NULL
                   GROUP BY s.directory
                   ORDER BY updated DESC";

        self.collect_rows(sql, &[], |row| {
            let path: String = row.get(0).map_err(query_err)?;
            Ok(DirectoryMeta {
                // directory 理论上非空，但历史数据里见过空串，别让它显示成空行。
                path: if path.is_empty() {
                    "(未知目录)".to_string()
                } else {
                    path
                },
                session_count: row.get(1).map_err(query_err)?,
                time_updated: row.get(2).map_err(query_err)?,
                project_count: row.get(3).map_err(query_err)?,
            })
        })
    }

    /// 模糊找出 worktree 含该子串的项目（返回 worktree 列表，已去重）。
    ///
    /// 去重是必要的：同路径的两个 project id 会产生两条相同 worktree，
    /// 而下游 `ProjectSelector::Like` 拿的是 worktree 而非 id。
    fn project_paths_like(&self, pattern: &str) -> Result<Vec<String>> {
        let sql = "SELECT DISTINCT worktree FROM project \
                   WHERE worktree LIKE ?1 ESCAPE '\\' ORDER BY worktree";
        let needle = format!("%{}%", escape_like(pattern));
        self.collect_rows(sql, &[&needle], |row| row.get(0).map_err(query_err))
    }

    fn sessions(&self, filter: &SessionFilter, sort: SortKey) -> Result<Vec<SessionMeta>> {
        let (where_clause, binds) = build_where(filter);
        let sql = format!(
            "SELECT {SESSION_COLUMNS} {SESSION_FROM_PROJECT}{where_clause} \
             ORDER BY {} LIMIT ?",
            order_by(sort)
        );

        let mut all: Vec<Box<dyn rusqlite::ToSql>> = binds;
        all.push(Box::new(filter.limit as i64));
        let refs: Vec<&dyn rusqlite::ToSql> = all.iter().map(|b| b.as_ref()).collect();

        self.collect_rows(&sql, &refs, session_from_row)
    }

    fn session(&self, session_id: &str) -> Result<Option<SessionMeta>> {
        let sql = format!("SELECT {SESSION_COLUMNS} {SESSION_FROM_PROJECT} WHERE s.id = ?1");
        Ok(self
            .collect_rows(&sql, &[&session_id], session_from_row)?
            .into_iter()
            .next())
    }

    /// 读取单个会话的全部消息，按 `seq` 升序。
    ///
    /// 坏 JSON 的消息被跳过并计入 [`MessageBatch::skipped`]，其余照常返回。
    fn messages(&self, session_id: &str) -> Result<MessageBatch> {
        let sql = "SELECT id, type, seq, time_created, data \
                   FROM session_message WHERE session_id = ?1 ORDER BY seq ASC";

        let mut stmt = self.conn.prepare(sql).map_err(query_err)?;
        let mut rows = stmt.query(params![session_id]).map_err(query_err)?;

        let mut entries = Vec::new();
        let mut skipped = 0usize;
        while let Some(row) = rows.next().map_err(query_err)? {
            match message_from_row(row) {
                Ok(m) => entries.push(m),
                Err(Error::MalformedJson { .. }) => skipped += 1,
                Err(e) => return Err(e),
            }
        }

        Ok(MessageBatch { entries, skipped })
    }

    fn message(&self, session_id: &str, seq: i64) -> Result<Option<MessageEntry>> {
        let sql = "SELECT id, type, seq, time_created, data \
                   FROM session_message WHERE session_id = ?1 AND seq = ?2";
        let mut stmt = self.conn.prepare(sql).map_err(query_err)?;
        let mut rows = stmt.query(params![session_id, seq]).map_err(query_err)?;

        match rows.next().map_err(query_err)? {
            Some(row) => Ok(Some(message_from_row(row)?)),
            None => Ok(None),
        }
    }
}
