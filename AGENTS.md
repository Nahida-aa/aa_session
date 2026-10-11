# aa_session

列出并导出本地 agent 会话。**v1 只做只读列举 + JSON 导出。**

## 定位：为什么不做搜索

核心场景已经定了——**全文检索交给用户自己的编辑器**。我们负责把会话导出成
可读 JSON，用户在 Zed 里搜。理由：

- 不必维护索引（曾建过 SQLite FTS5 侧车，470 MB 重复数据、永远可能过期、
  还要管 schema 版本；删掉后 891 个会话的列举是一次 SQL 查询的事）；
- 不必猜用户想要什么样的检索——Zed / IDE 的搜索比我们的 FTS 强得多；
- 导出物是**给人读的**：每条消息一个对象，`text` 字段直接是裸文本，
  Zed 搜「音频解码」能直接命中字符串本身。

代价：找东西要先知道大致在哪个会话（或先导出再搜）。这是明确的取舍。

## 命令

`--app` **必填**（放在子命令前后都行），不猜目标 app。

```bash
aa_session --app opencode projects   [--min-sessions N] [--json]
aa_session --app opencode directories [--directory SUB] [--min-sessions N] [--json]
aa_session --app opencode sessions   [--project ABS_PATH | --project-like SUB]
                                    [--directory SUB] [--agent NAME]...
                                    [--title SUB] [--since MINUTES]
                                    [--subagents exclude|include|only]
                                    [--archived exclude|only|include]
                                    [--sort updated|created|title|tokens] [--limit N] [--json]
aa_session --app opencode show   <SESSION_ID> [--with-tool-output] [--with-reasoning] [--max-tool-output N]
aa_session --app opencode export <SESSION_ID> [-o PATH] [--force] [导出选项同上]
```

- 表格里**不显示来源列**：`--app` 参数已经说明了是哪个 app。

### 项目寻址：`--project` 精确，`--project-like` 模糊

| 参数             | 语义                            | 输入                 | 多命中时                            |
| ---------------- | ------------------------------- | -------------------- | ----------------------------------- |
| `--project`      | **全等**匹配 `project.worktree` | 只接受**绝对路径**   | 永不歧义（自动聚合同路径的多个 id） |
| `--project-like` | **子串**匹配路径                | 任意子串（含目录名） | 正常全列，不报错                    |

两者互斥，同时给会报错。三条实现约束：

1. **不做 `canonicalize()`**。实测 12 个项目的目录已从磁盘删除，其下仍有
   **244 条会话**；`canonicalize` 会因路径不存在而失败，让这批会话永远查不到。
   只做去尾部 `/` 和展开 `~` 这类**字符串层面**的规范化。
2. **同路径多 id 必须聚合**。46 个项目行只有 45 个不同 worktree ——
   `Nahida-aa.github.io` 有**两个独立 id**（7 + 11 条会话）。
3. **`/` 是占位符不是真实路径**，显示为「（未知）」。opencode 为「无法确定所属
   仓库」的会话建了名为 `global` 的项目、worktree 填 `/`，那 10 条顶层会话散落
   在多个目录。用 `is_unknown_path()` 判断，别直接和 `"/"` 字符串比较。

### `projects` 与 `directories` 是两个视图，不是重复

**会话的运行目录（`session_v2.directory`）和它归属的项目（`project.worktree`）
是两件独立记录的事**，实测三类不一致：

| 现象                               | 实测                                                     |
| ---------------------------------- | -------------------------------------------------------- |
| 会话跑在项目仓库根的**子目录**     | 如 `/home/aa/repos/mc_ls/mcc/apps/web`                   |
| 会话跑在**另一个仓库**却归到原项目 | aaBot 有 5 条跑在 AAgent 目录                            |
| 同路径两个 project id              | `Nahida-aa.github.io` 在 `directories` 里显示 `项目数=2` |

所以「这个 agent 在哪工作过」只能用 `directories` 回答。`项目数` 列大于 1
即表示该目录混入了不属于本项目的会话。

### opencode 的三个路径字段（极易混淆，改代码前先看）

| 字段                   | 层级   | 含义             | 数据质量                                                                 |
| ---------------------- | ------ | ---------------- | ------------------------------------------------------------------------ |
| `project.worktree`     | 项目级 | git 仓库根       | 45/45 是绝对路径。`vcs` 列是**建项目时的快照**——目录删了也照样显示 `git` |
| `session_v2.directory` | 会话级 | 会话实际运行目录 | 891 行全绝对路径                                                         |
| `session_v2.path`      | 会话级 | 残留字段         | 仅 37 条有值且形态不一致（有缺首斜杠的），**直接忽略**                   |

`project.id` 是 40 字符哈希，**不等于路径**（实测 `id = sha1(path)` 不成立，
全库 0 行满足 `id = worktree`）。对外只用路径寻址，id 仅作内部 join 键。

### CLI 输出必须容忍 broken pipe

`println!` 遇写错误会 `panic!`，于是 `aa_session ... | head -5` 会炸出
`failed printing to stdout: Broken pipe`。已统一改用 `output::out()`：忽略写错误，
并**手动补换行**（`format_args!` 不像 `println!` 那样自动加换行）。

> `--app` 在 clap 里是「全局但可选」，靠代码校验必填。原因是 clap 的
> `global = true` + `required = true` 组合在子命令前后**都解析不到**，只会报一句
> 没有信息量的 "required arguments were not provided"；手动校验才能提示可用值。

### 默认只看「顶层 + 未归档」

两个维度都默认隐藏，都用显式开关放出来：

| 维度          | 现象                                                                                                                                                                                                                        | 默认      | 开关               |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------- | ------------------ |
| `--subagents` | opencode 把主 agent 派出的探查任务记成独立会话（`parent_id` 非空）。本库 891 个会话里 **652 个是子会话**（607 个是 `explore`），只有 239 个顶层；它们标题雷同、目录相同、各自几万 token，混在列表里会淹没真正想找的顶层会话 | `exclude` | `include` / `only` |
| `--archived`  | 用户主动收起的会话（`time_archived` 非空）。本库 17 条，其中 15 条在 YouDub-webui、1 条在 mcc                                                                                                                               | `exclude` | `include` / `only` |

`projects` 的三个计数列各自对齐口径（顶层 / 子会话 / 归档），三者互不重叠且都排除了
另一维度的过滤——否则 `projects` 显示 274 而同一项目 `sessions` 只列出 44 条，
用户会以为哪里坏了。已用一致性校验过（LocalDub: 44=44、YouDub-webui: 21=21）。

### 会话列表显示的字段

时间、tokens、agent、成本、标题/目录，附注行给子会话父 id、归档时间、改动文件数、
推理 token。

- **成本**：opencode 只对少数会话维护 `cost`（实测 891 条里仅 5 条非零），
  所以 `None`（显示 `-`）表示「上游没记录」，与「确实是 0 美元」严格区分。
- **`SessionMeta` 没有 `Eq`**：`cost` 是 `f64`，浮点不满足 `Eq`。为了显示真实成本
  放弃了整个类型的 `Eq` 绑定（本项目不需要把它当 `HashSet` key，可接受）。

## 日常操作用 justfile

`just`（https://github.com/casey/just）封装了常用命令，`just` 可列出全部配方：

| 配方                      | 用途                                                        |
| ------------------------- | ----------------------------------------------------------- |
| `just projects [过滤]`    | 列出项目（等价 `aa_session --app opencode projects`）       |
| `just sessions [过滤]`    | 列出会话                                                    |
| `just export <ID> [路径]` | 导出会话，不给路径时落到 `exports/<ID>.json`                |
| `just zed <ID> [路径]`    | 导出后直接用 Zed 打开                                       |
| `just find <模式>`        | 在 `exports/` 里全文搜索（等价于 Zed 的 cmd+f，但可脚本化） |
| `just ci`                 | `fmt --check` + `clippy` + `test`，提交前跑这个             |
| `just run -- <参数>`      | 透传参数给二进制                                            |

`exports/` 已 gitignore（单个导出可达数百 MB）。要用别的目录：
`AA_SESSION_EXPORTS=/path just find <模式>`。

### justfile 的两个坑（都踩过）

1. **just 只做文本替换，不创建同名 shell 变量。** 想用 shell 的默认值展开
   `${out:-...}`，必须先 `out="{{ out }}"` 赋值；直接写 `${out:-...}` 会**永远取
   默认值**——用户传了输出路径也不生效。同理 `$session_id` 也得先赋值，否则
   `set -u` 下报「未绑定的变量」。
2. **替换发生在 bash 看到脚本之前。** 插值进 shell 的参数一律用 `{{ quote(...) }}`
   转义，否则 `just find 'a$b'` 会被 bash 当变量、`just find '`id`'` 会被当命令
   **执行**。

另外：`export` 作为 recipe 名会让部分编辑器的 justfile 语法插件误报语法错误
（just 自身解析正常），故改名为 `dump`。

## 数据源

v1 **只有 opencode**，直读 `~/.local/share/opencode/opencode.db`（约 6 GB）。

- 读 `session_v2` / `session_message` / `project`（v2 schema）。
- v1 的 `session` / `message` / `part` **不读**——两套都在同一库里，但 v2 才是当前 schema。
- 以 SQLite `mode=ro` 打开，**永不写入**。

### 为什么直读而不是 `opencode serve` HTTP API

opencode 提供 `opencode serve`（`GET /session` 等）。不用它：

1. **它要求 opencode 进程在跑**，而核心场景恰恰是关着 opencode 时翻查历史；
2. 取全部消息正文要逐个 session 发 HTTP（891 个会话 / 20 万条消息），
   本地一次 SQL 即可；
3. 写路径本来就不该碰——那正是与运行中进程抢状态。

### 关于 `app`

`SessionSource::app()` 标识数据由哪个 agent 应用产生（opencode / claude-code /
codex）。它**同时是「来源」和「目标」**——将来 fork 时「从 app A 的会话 fork 出
app A 的新会话」用的是同一个字段。不要叫 `source`，那会被误解成只读导入方向。

## 架构

```
core/              领域模型 + SessionSource trait（零外部依赖）
sources/opencode/  opencode 只读实现（rusqlite）+ 导出格式
apps/cli/          clap 入口
```

依赖方向单向：`core` ← `sources/*` ← `apps/cli`。

**`core` 刻意不依赖 `rusqlite` / `anyhow`**——错误类型只带 `String` 描述，
库层用 `thiserror`，只有二进制入口用 `anyhow` 堆上下文。这样 GUI 阶段能直接
复用库而不必传递 `anyhow::Error`。

新增数据源（Claude Code 的 `~/.claude/projects/**/*.jsonl`、Codex 的
`~/.codex/sessions/**/*.jsonl`）只需再写一个 `SessionSource` 实现直读各自存储。

## 导出格式

```json
{
  "format_version": 1,
  "app": "opencode",
  "session": { "id": "...", "title": "...", "project_path": "...", ... },
  "skipped_messages": 0,
  "messages": [
    { "seq": 0, "id": "...", "role": "user", "time_created": 123,
      "text": "裸文本正文，供编辑器直接搜索",
      "blocks": [ { "type": "text", "text": "..." } ] }
  ]
}
```

- `text` 是把可读分块拼起来的裸文本 —— **为搜索而存在**。
- `blocks` 保留结构化信息（含 tool 调用的 id / name / status）。
- `seq` 是稳定标识，将来 fork 以它为边界。
- **tool 的 input / output 默认不导出**（单会话可达数百 MB，主要是文件全文和
  命令输出）；`--with-tool-output` 打开，`--max-tool-output N` 可截断。
- **reasoning 默认不导出**，`--with-reasoning` 打开。

实测：一个 1217 条消息的会话默认导出 810 KB，加全部 tool 输出后 33 MB。

## 🚫 硬约束：不写 opencode 库

**绝不对 `opencode.db` 做写操作。** 该库正被运行中的 opencode 进程持有，
写入会与其内存状态冲突并可能撞 WAL 锁。

具体地：

- **不做 import**。将来若要做，**必须默认先备份目标库**。
- **不在运行中的 opencode 库上测试任何写操作**。
- 要改内容 → 导出 JSON → 用户在编辑器里改。

（曾有一次在本项目里跑 `opencode import` 做往返测试，往真实库里写进了一个
空壳 session。虽然只污染了 v1 旧表的 1 行、不影响 891 个真实会话，但这是
不该发生的——测试写操作前必须先问用户。）

## 与 AAgent / zed 的关系

**零依赖，零复用。** 已核实：

- `packages/search`（16637 行）依赖 `editor`/`workspace`/`picker`，是 zed 面板不是库。
  唯一干净的是 `packages/project/src/search_history.rs`（128 行，纯 `std::VecDeque`，
  与 zed 逐字节相同）——将来 GUI 要搜索框翻历史时再抄。
- `packages/db` 是 zed 应用数据库框架（sqlez + `gpui::Global` + `inventory` 链接期
  migration），与本项目无关，**不用**。
- `packages/agent/src/db.rs:450` 把整个 thread 压成 zstd blob，没有可检索的行式存储。
  aacode 全仓库 0 处 FTS5/MATCH/LIKE，只在 `thread_search_bar.rs` 做内存线性扫描
  当前打开的 thread。
- `agent-client-protocol`（crates.io `=2.2.0`）是干净的公共 crate（9 个依赖，零 gpui），
  其 `ContentBlock` 的**分块思路**被 `core::ContentBlock` 借鉴，但 v1 不直接依赖它：
  opencode.db 的 JSON 是自己的格式，不是 ACP。
- ACP 已定义 `session/fork`（`unstable_session_fork`），但**只接受 `session_id`，
  没有边界 / seq 参数**——「从某个位置 fork」正是 ACP 的空白区。
  opencode 的 `session_v2` 虽有 `fork_session_id` / `fork_boundary` 字段，
  实测**全库 0 条使用**（652 条 `parent_id` 是子 agent 派生，不是 fork）。

## 下一步

1. `fork`（`--at <seq>`）—— fork-first，复制出新会话
2. 多数据源：Claude Code、Codex
3. 批量导出（`export --project X --all`），方便一次性倒进 Zed 搜
4. GUI（届时讨论复用 `gpui_learn` 还是自锁 zed rev）

## 开发

```bash
cargo test --workspace                     # 27 个测试
cargo clippy --workspace --all-targets     # 0 warning
cargo build --release
just ci                                    # fmt + clippy + test，提交前跑这个
```

## 提交与推送

agent 自行决定，准则如下：

- **提交**：每完成一个可独立理解的逻辑单元，且 `just ci` 全绿，就提交一条 commit；
  不积累大块未提交的工作。
  - 消息用 conventional 风格（按改动实质选 `feat:` / `fix:` / `refactor:` /
    `docs:` / `chore:`），描述可写中文。
  - 提交前必须 `just ci` 全绿（fmt + clippy 0 warning + test 全过），不绿不提交。
- **推送**：当工作已完整交付（各逻辑单元都已提交、ci 全绿）时，可自行 push 到
  `origin` 当前分支；半成品或重构中途的状态不 push。
  - 永远不 `--force`、不改写已推送的历史。

## 环境与可用工具

- **remote**：`origin` = `git@github.com:Nahida-aa/aa_session.git`。
- **`gh` CLI 已安装且已登录**（账号 `Nahida-aa`，git 协议 ssh）。
  GitHub 相关操作（看/建 issue、PR、release、查远端状态等）直接跑 `gh`，
  不需要网页或其他途径。

---

完整的通用 Rust 规则索引在 [`.agents/rust-skill-index.md`](.agents/rust-skill-index.md)
（cargo-skill 生成，与本项目无关，仅作参考）。
