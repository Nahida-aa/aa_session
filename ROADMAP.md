# 路线图

状态：`[x]` 已完成 · `[~]` 进行中 · `[ ]` 未开始

## v1 —— 只读列举与导出（当前阶段）

- [x] workspace 骨架（`core` / `sources/*` / `apps/cli`），零 zed/gpui 依赖
- [x] `--app` 必填参数 + 多数据源分派骨架（当前仅 `opencode`）
- [x] `projects` 列项目（会话 / 子会话 / 归档 三列各对齐口径）
- [x] `sessions` 列会话（按项目 / 目录 / agent / 标题 / 时间过滤，4 种排序）
- [x] `--project` 精确寻址（只认绝对路径，聚合同路径的多个 project id）
- [x] `--project-like` 模糊寻址（子串，多命中不报错）
- [x] `--subagents exclude|include|only`（默认排除子 agent 会话）
- [x] `--archived exclude|only|include`（默认排除归档会话）
- [x] `directories` 列出**会话运行目录**（与项目归属是两回事，独立视图）
- [x] `sessions` 同时显示会话目录与项目目录（不一致时才都列）
- [x] `show` / `export` 导出可读 JSON（裸文本 `text` 字段供编辑器搜索）
- [x] `rename-project` 项目改名（**唯一写库路径**，默认预检、写前强制备份）
- [x] justfile 日常命令封装
- [x] CLI 输出容忍 broken pipe
- [x] 表格输出改用 `comfy-table`（CJK 宽度对齐 + 路径左截断），一条记录一行
- [x] `--format table|json|toon`（TOON 省 48% token，人也能读；`--json` 保留为简写）

## v2 —— 写操作（fork / edit）

> 前置约束：**fork-first**，编辑永远作用在副本上，原库只读。
> 写操作一律「默认预检 + `--apply` 执行 + 写前备份」三件套，
> 并在 README 与 `--help` 中提示「先关闭 opencode」。

- [ ] `fork` —— `--at <seq>` 从指定位置截断复制出新会话
  - [ ] 确认 opencode v2 schema 下新 session / session_message 的必填字段
  - [ ] `session_v2.fork_session_id` / `fork_boundary` 的填充（**当前全库 0 条使用**，
        字段语义要实测确认，不能想当然）
  - [ ] 预检模式：打印将复制的消息条数与体积
  - [ ] 事务 + 备份
- [ ] `edit` —— 基于 fork 出的副本编辑
  - [ ] 交互式编辑（`$EDITOR`）还是接受外部文件路径？
  - [ ] 编辑后校验（seq 连续性 / JSON schema）
- [ ] 写操作的统一抽象（现在只有 rename 一条路径，fork/edit 应复用同一套
      备份 + 预检 + 事务骨架，而不是各写一遍）

## v3 —— 多数据源

- [ ] `SessionSource` 的第二个实现（架构已就位，代码未写）
  - [ ] Claude Code：`~/.claude/projects/**/*.jsonl`
  - [ ] Codex：`~/.codex/sessions/**/*.jsonl`
  - [ ] 评估两者是否也需要「项目 / 目录」双视图（Claude Code 按项目目录分文件，
        可能天然不需要 `directories`）
- [ ] `--app` 枚举扩展（`AppArg` 加变体即可）

## v4 —— GUI

- [ ] 讨论 GPUI 来源：`gpui_learn` 的锁定 rev vs 自锁 zed rev
- [ ] 复用 `aa_gpui_kit_ui` 还是自建组件层
- [ ] 搜索框翻历史（可直接抄 `packages/project/src/search_history.rs`，
      128 行纯 `std::VecDeque`，与 zed 逐字节相同）

## 已明确不做

- [ ] **全文检索索引** —— 检索交给用户在编辑器里做，导出物每条消息带裸文本
      `text` 字段。曾建过 SQLite FTS5 侧车，因检索需求取消而整体删除
      （省下 470 MB 重复数据与 schema 版本管理）
- [ ] **import** —— 用户明确要求不做；若将来要做，**必须默认先备份**，
      且不得在运行中的 opencode 库上测试
- [ ] **自动增量同步** —— 列举 891 个会话本来就是一次 SQL 查询，保持实时可读

## 技术债

- [ ] `SessionMeta.model` 是 `Option<String>`，里面塞的是 **JSON 字符串**
      （`{"id":"big-pickle","providerID":"opencode"}`）。它在 json 输出里是一坨
      转义文本，在 toon 输出里也一样，白白吃掉省下来的 token。应改成
      `Option<Model>` 结构体或 `serde_json::Value`。

- [ ] 导出时 snippet 之外，`--with-tool-output` 的体积没有硬上限
      （单会话实测可达 33 MB）
- [ ] `session_v2.path` 字段确认：仅 37 条有值且形态不一致，当前直接忽略；
      需确认是否某个 opencode 版本在用它
- [ ] opencode 1.19.0 已在本机出现（Zed external agents 拉取），
      本项目全部 schema 分析基于 1.18.35，需要复核