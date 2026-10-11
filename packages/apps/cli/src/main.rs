//! `aa_session` —— 列出并导出本地 agent 会话。
//!
//! 设计前提：**不做全文搜索**。891 个会话的列举是一次 SQL 查询的事，而全文
//! 检索交给用户在自己喜欢的编辑器里做——先把会话导出成可读 JSON，再用 Zed /
//! IDE 的搜索。这样我们不必维护索引，也不必猜用户想要什么样的检索。

mod output;

use output::{human_bytes, out};

use std::path::PathBuf;

use aa_session_core::{
    Archived, Error, ProjectSelector, SessionFilter, SessionSource, SortKey, Subagents,
};
use aa_session_source_opencode::OpencodeSource;
use aa_session_source_opencode::export::{ExportOptions, build};
use aa_session_source_opencode::{backup, rename};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

/// 顶层参数。
#[derive(Debug, Parser)]
#[command(
    name = "aa_session",
    version,
    about = "列出并导出本地 agent 会话",
    before_help = BEFORE_HELP
)]
struct Args {
    /// 操作哪个 agent 应用的数据（必填，不猜）。
    ///
    /// 这里不能用 clap 的 `required`：全局必填参数在 clap 里两种位置都解析不到，
    /// 只能写成可选 + 下面手动校验，好处是能给出可操作的错误提示。
    // 只给长选项：`-a` 已分配给 `sessions --agent`，clap 的 debug_assert
    // 会在两个短选项冲突时直接 panic（只有同时用到两个参数才触发，
    // 很容易漏测）。
    #[arg(long, global = true, value_enum)]
    app: Option<AppArg>,

    /// 覆盖该 app 的数据库路径（默认由 app 自身决定）。
    #[arg(long, global = true, value_name = "PATH")]
    db: Option<PathBuf>,

    /// 日志级别（`RUST_LOG` 同格式，默认 `warn`）。
    #[arg(long, global = true, value_name = "FILTER")]
    log: Option<String>,

    #[command(subcommand)]
    command: Command,
}

const BEFORE_HELP: &str = "\
`--app` 是必填的 —— 本工具只读某个 agent 应用自己的存储，不猜目标。

典型流程
  1. 看看有哪些项目          aa_session --app opencode projects
  2. 挑一个项目看会话        aa_session --app opencode sessions --project LocalDub
  3. 导出会话                aa_session --app opencode export <SESSION_ID> -o chat.json
  4. 在编辑器里全文搜索      zed chat.json        （或 code / vim / 任何工具）

导出的 JSON 每条消息带一个裸文本 `text` 字段，可直接被编辑器的搜索命中。
工具调用的 input/output 默认不导出（体积可达数百 MB），需要时加
--with-tool-output。

默认只列出**顶层**且**未归档**的会话。opencode 会把派出的探查任务记成子会话
（本库 891 个会话里 652 个是子会话），它们默认隐藏，需要时用
--subagents include|only。同理归档会话默认隐藏，用 --archived include|only。

本工具对 opencode 数据库**只读**，不会写入任何内容。
";

#[derive(Debug, Subcommand)]
enum Command {
    /// 列出所有项目及其会话数。
    Projects {
        /// 只列会话数 >= 该值的项目。
        #[arg(short = 'n', long, value_name = "N")]
        min_sessions: Option<i64>,

        /// 以 JSON 输出。
        #[arg(long)]
        json: bool,
    },

    /// 列出会话。
    Sessions {
        /// 最多列出多少条。
        #[arg(short, long, default_value_t = 30)]
        limit: usize,

        /// 只列该项目的会话。**只接受绝对路径，全等匹配** `project.worktree`。
        ///
        /// 模糊匹配（目录名、部分路径）请用 `--project-like`。
        #[arg(long = "project", value_name = "ABS_PATH")]
        project: Option<String>,

        /// 按项目路径**子串**模糊过滤（多命中是预期行为）。
        ///
        /// 与 `--project` 互斥。
        #[arg(long = "project-like", value_name = "SUBSTR")]
        project_like: Option<String>,

        /// 只列工作目录包含该子串的会话。
        #[arg(long, value_name = "SUBSTR")]
        directory: Option<String>,

        /// 只列这些 agent，可重复（如 build / plan）。
        #[arg(short, long = "agent", value_name = "NAME")]
        agents: Vec<String>,

        /// 只列标题包含该子串的会话（大小写不敏感）。
        #[arg(short = 't', long = "title", value_name = "SUBSTR")]
        title: Option<String>,

        /// 只列最近 N 分钟内更新过的会话。
        #[arg(long, value_name = "MINUTES")]
        since: Option<i64>,

        /// 子 agent 会话（opencode 派出的探查任务）的纳入方式。
        #[arg(long, value_enum, default_value_t = SubArg::Exclude, value_name = "MODE")]
        subagents: SubArg,

        /// 归档会话的纳入方式。归档是用户主动收起的会话，默认不显示。
        #[arg(long, value_enum, default_value_t = ArchArg::Exclude, value_name = "MODE")]
        archived: ArchArg,

        /// 排序键。
        #[arg(long, value_enum, default_value_t = SortArg::Updated)]
        sort: SortArg,

        /// 以 JSON 输出。
        #[arg(long)]
        json: bool,
    },

    /// 把项目的路径改成新的（**唯一会写 opencode 库的子命令**）。
    ///
    /// 默认只做预检并打印将要发生的改动，加 `--apply` 才真正写入。
    /// 仅改「指针」字段（`project.worktree`、`session.directory`），
    /// **不改**消息历史里的路径（那是当时实际发生的事）。
    RenameProject {
        /// 旧路径（绝对路径，全等匹配）。
        old_path: String,

        /// 新路径（绝对路径）。
        new_path: String,

        /// 真正写入。不加这个只做预检。
        #[arg(long)]
        apply: bool,

        /// 允许新路径在磁盘上不存在（默认要求存在，避免打错字）。
        #[arg(long)]
        allow_missing: bool,

        /// 备份文件路径（默认与数据库同目录）。
        #[arg(long, value_name = "PATH")]
        backup_to: Option<PathBuf>,
    },

    /// 列出所有**会话运行目录**及其会话数。
    ///
    /// 会话的运行目录和它归属的项目**不是一回事**（见下方说明），所以这是
    /// 一个独立视图，而不是 `projects` 的重复。
    Directories {
        /// 只列包含该子串的目录。
        #[arg(long, value_name = "SUBSTR")]
        directory: Option<String>,

        /// 只列会话数 >= 该值的目录。
        #[arg(short = 'n', long, value_name = "N")]
        min_sessions: Option<i64>,

        /// 以 JSON 输出。
        #[arg(long)]
        json: bool,
    },

    /// 把一个会话导出为 JSON 到标准输出。
    ///
    /// 不带 `-o` 时写到 stdout，可直接管道给 `jq` 或重定向到文件。
    Show {
        /// 会话 id。
        session_id: String,

        #[command(flatten)]
        export: ExportArgs,
    },

    /// 把一个会话导出为 JSON 文件。
    Export {
        /// 会话 id。
        session_id: String,

        /// 输出路径。缺省写到 `./<session_id>.json`。
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,

        /// 覆盖已存在的输出文件。
        #[arg(short, long)]
        force: bool,

        #[command(flatten)]
        export: ExportArgs,
    },
}

/// `show` / `export` 共用的导出选项。
#[derive(Debug, clap::Args)]
struct ExportArgs {
    /// 导出工具调用的 input / output。会让文件体积暴涨（单会话可达数百 MB）。
    #[arg(long)]
    with_tool_output: bool,

    /// 导出助手的推理内容。
    #[arg(long)]
    with_reasoning: bool,

    /// 每条工具输出最多保留多少字符（配合 `--with-tool-output`）。
    #[arg(long, value_name = "N")]
    max_tool_output: Option<usize>,
}

/// 要操作的 agent 应用。
///
/// 目前只实现了 opencode；将来加 Claude Code / Codex 时在这里加变体即可，
/// 其余代码不需要改。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AppArg {
    /// opencode（`~/.local/share/opencode/opencode.db`）。
    Opencode,
}

impl AppArg {
    /// 该 app 的默认数据库路径。
    fn default_db_path(self) -> PathBuf {
        match self {
            AppArg::Opencode => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default();
                home.join(aa_session_source_opencode::DEFAULT_DATA_DIR)
                    .join(aa_session_source_opencode::DB_FILE_NAME)
            }
        }
    }

    /// 按 `--app` 分派到对应的数据源实现。
    fn open(self, db: Option<&std::path::Path>) -> Result<Box<dyn SessionSource>> {
        let path = db
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.default_db_path());
        match self {
            AppArg::Opencode => Ok(Box::new(OpencodeSource::open(&path).with_context(
                || {
                    format!(
                        "无法打开 {} 的数据库（路径 {}）",
                        self.name(),
                        path.display()
                    )
                },
            )?)),
        }
    }

    /// 该 app 的显示名。
    fn name(self) -> &'static str {
        match self {
            AppArg::Opencode => "opencode",
        }
    }
}

/// 归档会话的纳入方式（对应 `aa_session_core::Archived`）。
#[derive(Debug, Clone, Copy, ValueEnum)]
enum ArchArg {
    /// 排除归档会话（默认）。
    Exclude,
    /// 只显示归档会话。
    Only,
    /// 归档与未归档都显示。
    Include,
}

impl From<ArchArg> for Archived {
    fn from(v: ArchArg) -> Self {
        match v {
            ArchArg::Exclude => Archived::Exclude,
            ArchArg::Only => Archived::Only,
            ArchArg::Include => Archived::Include,
        }
    }
}

/// 子 agent 会话的纳入方式（对应 `aa_session_core::Subagents`）。
#[derive(Debug, Clone, Copy, ValueEnum)]
enum SubArg {
    /// 只显示顶层会话（默认）。
    Exclude,
    /// 顶层与子会话都显示。
    Include,
    /// 只显示子会话。
    Only,
}

impl From<SubArg> for Subagents {
    fn from(v: SubArg) -> Self {
        match v {
            SubArg::Exclude => Subagents::Exclude,
            SubArg::Include => Subagents::Include,
            SubArg::Only => Subagents::Only,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SortArg {
    /// 按最后更新时间倒序（默认）。
    Updated,
    /// 按创建时间倒序。
    Created,
    /// 按标题排序。
    Title,
    /// 按总 token 数倒序。
    Tokens,
}

impl From<SortArg> for SortKey {
    fn from(v: SortArg) -> Self {
        match v {
            SortArg::Updated => SortKey::UpdatedDesc,
            SortArg::Created => SortKey::CreatedDesc,
            SortArg::Title => SortKey::Title,
            SortArg::Tokens => SortKey::Tokens,
        }
    }
}

fn main() -> Result<()> {
    let cli = Args::parse();

    // `--app` 必填。手动校验而不是交给 clap，是为了让报错告诉用户有哪些选项，
    // 而不是干巴巴一句 "required arguments were not provided"。
    let app = cli.app.ok_or_else(|| {
        anyhow::anyhow!(
            "必须用 --app 指定要操作的 agent 应用。\n\
             可用：{}",
            AppArg::value_variants()
                .iter()
                .filter_map(|v| v.to_possible_value().map(|p| p.get_name().to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let filter = match &cli.log {
        Some(spec) => EnvFilter::new(spec),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let source = app.open(cli.db.as_deref())?;

    match cli.command {
        Command::Projects { min_sessions, json } => {
            let projects = source.projects()?;
            let projects: Vec<_> = projects
                .into_iter()
                .filter(|p| min_sessions.is_none_or(|n| p.session_count >= n))
                .collect();
            output::print_projects(&projects, json)?;
        }

        Command::RenameProject {
            old_path,
            new_path,
            apply,
            allow_missing,
            backup_to,
        } => {
            run_rename_project(
                app,
                cli.db.as_ref(),
                &old_path,
                &new_path,
                apply,
                allow_missing,
                backup_to,
            )?;
        }

        Command::Directories {
            directory,
            min_sessions,
            json,
        } => {
            let dirs = source.directories()?;
            let needle = directory.map(|d| d.to_lowercase());
            let dirs: Vec<_> = dirs
                .into_iter()
                .filter(|d| min_sessions.is_none_or(|n| d.session_count >= n))
                .filter(|d| {
                    needle
                        .as_ref()
                        .is_none_or(|n| d.path.to_lowercase().contains(n))
                })
                .collect();
            output::print_directories(&dirs, json)?;
        }

        Command::Sessions {
            limit,
            project,
            project_like,
            directory,
            agents,
            title,
            since,
            subagents,
            archived,
            sort,
            json,
        } => {
            if project.is_some() && project_like.is_some() {
                anyhow::bail!(
                    "--project（精确）与 --project-like（模糊）互斥，请只用一个。\n\
                     精确匹配要绝对路径；模糊匹配才是子串。"
                );
            }

            let selector = match (&project, &project_like) {
                (Some(p), _) => ProjectSelector::Exact {
                    path: resolve_exact_project(source.as_ref(), p)?,
                },
                (_, Some(pattern)) => ProjectSelector::Like {
                    pattern: pattern.clone(),
                    paths: source.project_paths_like(pattern)?,
                },
                _ => ProjectSelector::All,
            };

            if let ProjectSelector::Like { paths, pattern } = &selector
                && paths.is_empty()
            {
                // 空结果必须提示，否则用户会以为是「没有会话」而不是「没有项目」。
                anyhow::bail!(
                    "没有路径包含 `{pattern}` 的项目；\
                     先跑 `aa_session --app opencode projects` 看看有哪些。"
                );
            }

            let filter = SessionFilter {
                project: selector,
                directory,
                agents,
                title_contains: title,
                // 「最近 N 分钟」在 SQL 里比较，不把全表拉进内存。
                updated_after: since.map(minutes_ago_ms),
                subagents: subagents.into(),
                archived: archived.into(),
                limit,
            };

            let sessions = source.sessions(&filter, sort.into())?;
            output::print_sessions(&sessions, json)?;
        }

        Command::Show {
            session_id,
            export: args,
        } => {
            let file = assemble(source.as_ref(), &session_id, &args)?;
            out(format_args!("{}", serde_json::to_string_pretty(&file)?));
        }

        Command::Export {
            session_id,
            output: output_path,
            force,
            export: args,
        } => {
            let file = assemble(source.as_ref(), &session_id, &args)?;

            let path = output_path.unwrap_or_else(|| PathBuf::from(format!("{session_id}.json")));
            if path.exists() && !force {
                anyhow::bail!(
                    "{} 已存在；加 --force 覆盖，或用 -o 指定别的路径",
                    path.display()
                );
            }

            let text = serde_json::to_string_pretty(&file)?;
            std::fs::write(&path, &text).map_err(|e| Error::Io {
                path: path.display().to_string(),
                cause: e.to_string(),
            })?;

            out(format_args!(
                "已导出 {} 条消息（{} 字节）→ {}",
                file.messages.len(),
                text.len(),
                path.display()
            ));
        }
    }

    Ok(())
}

/// 把 `--project` 的输入规范成一个**库中真实存在的绝对路径**。
///
/// 三条规则，都是为了防错：
///
/// 1. **必须是绝对路径**。相对路径一律拒绝并给出建议 —— 库里的 worktree
///    全是绝对路径（实测 45/45），相对路径必然匹配不上。
/// 2. **不做 `canonicalize()`**。实测 12 个项目的目录已从磁盘删除，其下仍有
///    **244 条会话**；`canonicalize` 会因为路径不存在而失败，让这批会话永远
///    查不到。只做去尾部斜杠 + 展开 `~` 这类「字符串层面」的规范化。
/// 3. **尾部 `/` 去掉**。`/a/b/` 和 `/a/b` 在库里是同一个项目。
fn resolve_exact_project(source: &dyn SessionSource, input: &str) -> Result<String> {
    // `~` 展开：用户在 shell 里可能没引号，`~/foo` 会被 shell 自己展开，
    // 但 `--project '~/foo'` 传到我们这儿还是带波浪号的。
    let mut path = if let Some(rest) = input.strip_prefix("~/") {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(rest).display().to_string())
            .unwrap_or_else(|_| input.to_string())
    } else {
        input.to_string()
    };

    // 去掉尾部斜杠，但保留根目录 "/" 本身。
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }

    if !path.starts_with('/') {
        anyhow::bail!(
            "--project 只接受绝对路径，收到的是 `{input}`。\n\
             模糊匹配请用 --project-like {input}"
        );
    }

    // `global` 兜底项目的 worktree 就是 "/"，命中它要提醒 —— 用户多半是想查
    // 某个具体目录，而不是这 24 条散落 6 处的会话。
    if path == aa_session_core::UNKNOWN_PROJECT_PATH {
        // 会话数取自 `projects()` 的口径（顶层 + 未归档），而不是再跑一次
        // sessions —— `SessionFilter::default()` 的 limit 是 0，拿它计数会得到 0。
        let n = source
            .projects()?
            .into_iter()
            .find(|p| p.is_unknown_path())
            .map(|p| p.session_count)
            .unwrap_or(0);
        eprintln!(
            "提示：`/` 是 opencode 对「无法确定所属仓库」的占位值，对应 {n} 条顶层会话，散落在多个目录里。"
        );
        eprintln!("要看具体目录，用 `aa_session --app opencode directories`。");
    }

    // 存在性检查放在解析之后 —— 用 `projects()` 校验而不是额外的 SQL。
    // 同路径多 id 会返回多行，那正是要聚合的情况，不算错。
    let known = source
        .projects()?
        .into_iter()
        .any(|p| normalize_path(&p.path) == path);
    if !known {
        anyhow::bail!(
            "库中没有 worktree 为 `{path}` 的项目。\n\
                        先跑 `aa_session --app opencode projects` 看看有哪些。"
        );
    }

    Ok(path)
}

/// 路径字符串层面的规范化（与 `resolve_exact_project` 保持一致）。
fn normalize_path(path: &str) -> String {
    let mut p = path.to_string();
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    p
}

/// 「N 分钟前」的 Unix 毫秒时间戳。
fn minutes_ago_ms(minutes: i64) -> i64 {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    now_ms - minutes * 60 * 1000
}

/// 取出会话 + 消息并组装成导出结构。
fn assemble(
    source: &dyn SessionSource,
    session_id: &str,
    args: &ExportArgs,
) -> Result<aa_session_source_opencode::export::ExportFile> {
    let session = source
        .session(session_id)?
        .ok_or_else(|| Error::NotFound(format!("session {session_id}")))?;

    let batch = source.messages(session_id)?;

    if batch.skipped > 0 {
        tracing::warn!(
            session = session_id,
            skipped = batch.skipped,
            "some messages had malformed json and were skipped"
        );
    }

    let opts = ExportOptions {
        with_tool_output: args.with_tool_output,
        with_reasoning: args.with_reasoning,
        max_tool_output_chars: args.max_tool_output,
    };

    Ok(build(&session, &batch, &opts))
}

/// 改名子命令的实现。
///
/// 安全设计（这是本工具唯一写库的路径，规则必须硬）：
///
/// 1. **默认只预检**。没有 `--apply` 就只打印将要发生的改动。
/// 2. **写前强制备份**，且备份已存在时拒绝执行。
/// 3. **磁盘存在性校验**，除非 `--allow-missing`。
/// 4. 目标路径**不能被其他项目占用**。
#[allow(clippy::too_many_arguments)]
fn run_rename_project(
    app: AppArg,
    db_override: Option<&PathBuf>,
    old_path: &str,
    new_path: &str,
    apply: bool,
    allow_missing: bool,
    backup_to: Option<PathBuf>,
) -> Result<()> {
    // 复用精确寻址的规范化（去尾部 /、展开 ~），保证和查询时同一套规则。
    let old_path = normalize_cli_path(old_path)?;
    let new_path = normalize_cli_path(new_path)?;

    if old_path == new_path {
        anyhow::bail!("新旧路径相同，无需改名");
    }

    for (label, p) in [("旧", &old_path), ("新", &new_path)] {
        if !p.starts_with('/') {
            anyhow::bail!("{label}路径 `{p}` 不是绝对路径");
        }
    }

    // 跟随 `--db`：用户显式给了路径就是想操作那个库，而且这样才能在副本上
    // 做写入演练。默认值仍是 app 的标准位置。
    let db_path = db_override
        .cloned()
        .unwrap_or_else(|| app.default_db_path());

    // 预检用只读连接。
    let ro = OpencodeSource::open(&db_path)?;
    let old_ids = ro.project_ids_for_path(&old_path)?;
    if old_ids.is_empty() {
        anyhow::bail!(
            "库中没有 worktree 为 `{old_path}` 的项目；\
             先跑 `aa_session --app {} projects` 看看有哪些。",
            app.name()
        );
    }

    // 目标不能被别的项目占用（同路径多个 id 是历史遗留，不算冲突）。
    let new_ids = ro.project_ids_for_path(&new_path)?;
    if !new_ids.is_empty() {
        anyhow::bail!(
            "路径 `{new_path}` 已被 {} 个项目占用，拒绝改名以免合并到一起",
            new_ids.len()
        );
    }

    // 磁盘存在性：新路径必须真的在那儿，否则多半是打错字。
    if !allow_missing && !std::path::Path::new(&new_path).exists() {
        anyhow::bail!(
            "新路径 `{new_path}` 在磁盘上不存在。\n\
             如果这是预期的（例如目录还没建、或者它在另一台机器上），加 --allow-missing。"
        );
    }

    let plan = rename::plan(ro.conn(), &old_path, &new_path)?;

    out(format_args!("项目改名预检  {} → {}", old_path, new_path));
    out(format_args!(
        "  project.worktree          {} 行",
        plan.project_rows
    ));
    out(format_args!(
        "  session_v2.directory       {} 行",
        plan.session_v2_rows
    ));
    out(format_args!(
        "  session.directory (v1)    {} 行",
        plan.session_rows
    ));
    out(format_args!(
        "  消息历史内的路径           0 行（刻意不改，见 AGENTS.md）"
    ));
    out(format_args!(
        "  合计改动                   {} 行",
        plan.total()
    ));

    if plan.is_noop() {
        out(format_args!("\n没有任何记录需要改动。"));
        return Ok(());
    }

    if !apply {
        out(format_args!("\n这是预检。确认无误后加 --apply 执行。"));
        return Ok(());
    }

    // ---- 以下才真正写库 ----
    eprintln!("\n⚠ 即将写入 opencode 数据库：{}", db_path.display());
    eprintln!("⚠ 请确认 opencode 没有在运行，否则它内存里的会话状态会与磁盘不一致。");

    let dest = backup_to.unwrap_or_else(|| backup::backup_path_for(&db_path));
    let mut rw = OpencodeSource::open_writable(&db_path)?;

    let saved = backup::backup(rw.conn(), &dest)?;
    let size = std::fs::metadata(&saved).map(|m| m.len()).unwrap_or(0);
    out(format_args!(
        "\n已备份 → {}（{}）",
        saved.display(),
        human_bytes(size)
    ));

    let applied = rename::apply(rw.conn_mut(), &old_path, &new_path)?;
    out(format_args!(
        "已改名：{} 行。备份保留在 {}",
        applied.total(),
        saved.display()
    ));
    out(format_args!(
        "撤销方式：用备份文件替换主库（先关闭 opencode）。"
    ));

    Ok(())
}

/// `--project` / `rename-project` 共用的路径规范化。
fn normalize_cli_path(input: &str) -> Result<String> {
    let mut path = if let Some(rest) = input.strip_prefix("~/") {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(rest).display().to_string())
            .unwrap_or_else(|_| input.to_string())
    } else {
        input.to_string()
    };
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    Ok(path)
}
