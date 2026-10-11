//! Codebuddy 会话数据源（只读，直接扫 JSONL 文件）。
//!
//! 会话存在 `~/.codebuddy/projects/<编码路径>/<uuid>.jsonl`，每个文件是一条
//! 会话的事件流。`projects` 目录下还有 `memory` 子目录等非会话内容，跳过。
//!
//! # 路径编码规则
//!
//! `/home/aa/repos/ai_ls/AAgent` → 目录名 `home-aa-repos-ai_ls-AAgent`
//! （把 `/` 换成 `-`，去掉开头的 `/`）。反推时直接 `"/" + name.split("-")`，
//! 但路径段本身可能含 `-`（如 `ai_ls`），所以用 `projects` 目录枚举 +
//! 反向查找，而不是机械地按 `-` 拆分。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aa_session_core::{
    DirectoryMeta, Error, ProjectMeta, ProjectSelector, Result,
    SessionFilter, SessionMeta, SessionSource, SortKey,
};

pub const APP_NAME: &str = "codebuddy";
pub const DEFAULT_DATA_DIR: &str = ".codebuddy";
pub const PROJECTS_DIR: &str = "projects";

/// Codebuddy 的只读数据源（JSONL 文件，无数据库）。
pub struct CodebuddySource {
    root: PathBuf,
}

impl CodebuddySource {
    /// 从 `$HOME` 展开后的默认位置打开。
    pub fn open_default() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::InvalidArgument("HOME is not set".into()))?;
        Self::open_dir(&home.join(DEFAULT_DATA_DIR))
    }

    /// 从数据目录打开（如 `~/.codebuddy`）。
    pub fn open_dir(dir: &Path) -> Result<Self> {
        if !dir.exists() {
            return Err(Error::Open {
                path: dir.display().to_string(),
                cause: "directory does not exist".into(),
            });
        }
        Ok(Self {
            root: dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()),
        })
    }

    /// 项目根目录（`projects/`）。
    fn projects_dir(&self) -> PathBuf {
        self.root.join(PROJECTS_DIR)
    }

    /// 遍历 `projects/` 下所有 `.jsonl` 会话文件。
    fn list_session_files(&self) -> Result<Vec<PathBuf>> {
        let projects_dir = self.projects_dir();
        if !projects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        let entries = fs::read_dir(&projects_dir)
            .map_err(|e| Error::Open {
                path: projects_dir.display().to_string(),
                cause: e.to_string(),
            })?;

        for entry in entries.flatten() {
            let proj_dir = entry.path();
            if !proj_dir.is_dir() {
                continue;
            }
            if let Ok(sessions) = fs::read_dir(&proj_dir) {
                for s in sessions.flatten() {
                    let p = s.path();
                    if p.extension().map(|e| e == "jsonl").unwrap_or(false)
                        && !p.is_dir()
                    {
                        files.push(p);
                    }
                }
            }
        }
        Ok(files)
    }
    /// 按 session_id 找到该会话的 JSONL 文件。
    ///
    /// session_id 等于文件名（去掉 `.jsonl`），先按文件名精确匹配。
    fn find_session_file(&self, session_id: &str) -> Result<PathBuf> {
        let projects_dir = self.projects_dir();
        let entries = fs::read_dir(&projects_dir).map_err(|e| Error::Open {
            path: projects_dir.display().to_string(),
            cause: e.to_string(),
        })?;

        for proj in entries.flatten() {
            let proj_dir = proj.path();
            if !proj_dir.is_dir() {
                continue;
            }
            let candidate = proj_dir.join(format!("{session_id}.jsonl"));
            if candidate.exists() {
                return Ok(candidate);
            }
        }

        Err(Error::NotFound(format!("session {session_id}")))
    }

}

/// 读一个 JSONL 文件的全部事件（跳过空行与坏 JSON）。
fn read_events(path: &Path) -> Result<Vec<serde_json::Value>> {
    let content = fs::read_to_string(path).map_err(|e| Error::Open {
        path: path.display().to_string(),
        cause: e.to_string(),
    })?;

    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(v) => events.push(v),
            Err(_) => {
                tracing::warn!(file = %path.display(), "skipping malformed JSONL line");
            }
        }
    }
    Ok(events)
}

/// 把事件流压缩成一条会话的元数据。
///
/// `file_stem` 是 JSONL 文件名（不含扩展名）。没有 `session-meta` 的旧文件
/// 里 `sessionId` 可能缺失，用它兜底，让空壳会话（0 条 message）也能出现
/// 在 `--empty include` 的列表里。
fn summarize(
    events: &[serde_json::Value],
    project_id: &str,
    project_path: &str,
    file_stem: &str,
) -> Option<SessionMeta> {
    let mut session_id = String::new();
    let mut title = String::new();
    let mut directory = String::new();
    let mut time_created: i64 = 0;
    let mut time_updated: i64 = 0;
    let mut tokens_input: i64 = 0;
    let mut tokens_output: i64 = 0;

    for ev in events {
        match ev.get("type").and_then(|t| t.as_str()) {
            Some("session-meta") => {
                if session_id.is_empty() {
                    session_id = ev
                        .get("sessionId")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
            Some("ai-title") => {
                if title.is_empty() {
                    title = ev
                        .get("aiTitle")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    directory = ev
                        .get("cwd")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(ts) = ev.get("timestamp").and_then(|t| t.as_i64()) {
                        time_updated = ts;
                    }
                }
            }
            Some("message") => {
                let ts = ev.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                if time_created == 0 || ts < time_created {
                    time_created = ts;
                }
                if ts > time_updated {
                    time_updated = ts;
                }
                if ev.get("role").and_then(|r| r.as_str()) == Some("user") {
                    if title.is_empty()
                        && let Some(first) = ev
                            .get("content")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|b| b.get("text"))
                            .and_then(|t| t.as_str())
                    {
                        let trimmed: String = first.chars().take(80).collect();
                        title = trimmed.replace('\n', " ");
                    }
                    if directory.is_empty() {
                        directory = ev
                            .get("cwd")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                }
                if let Some(tokens) = ev
                    .get("__codebuddyLocal")
                    .and_then(|l| l.get("sens"))
                    .and_then(|s| s.get("tokens"))
                {
                    tokens_input += tokens.get("input").and_then(|t| t.as_i64()).unwrap_or(0);
                    tokens_output += tokens.get("output").and_then(|t| t.as_i64()).unwrap_or(0);
                }
            }
            Some("turn-metrics") => {
                if let Some(delta) = ev.get("tokenDelta").and_then(|d| d.as_i64()) {
                    tokens_output += delta;
                }
            }
            _ => {}
        }
    }

    // 没有 `session-meta` 的文件里 `sessionId` 可能缺失，用文件名兜底。
    if session_id.is_empty() {
        session_id = file_stem.to_string();
    }
    if session_id.is_empty() {
        return None;
    }

    Some(SessionMeta {
        app: APP_NAME.to_string(),
        id: session_id,
        project_id: project_id.to_string(),
        project_path: project_path.to_string(),
        parent_id: None,
        title,
        directory,
        agent: None,
        model: None,
        time_created,
        time_updated,
        tokens_input,
        tokens_output,
        tokens_reasoning: 0,
        cost: None,
        files_changed: 0,
        archived_at: None,
    })
}

impl SessionSource for CodebuddySource {
    fn app(&self) -> &'static str {
        APP_NAME
    }

    fn projects(&self) -> Result<Vec<ProjectMeta>> {
        let projects_dir = self.projects_dir();
        if !projects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut out: Vec<ProjectMeta> = Vec::new();
        let entries = fs::read_dir(&projects_dir).map_err(|e| Error::Open {
            path: projects_dir.display().to_string(),
            cause: e.to_string(),
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let proj_name = entry.file_name().to_string_lossy().to_string();

            let session_files: Vec<PathBuf> = fs::read_dir(&path)
                .map(|entries| {
                    entries
                        .into_iter()
                        .flatten()
                        .filter_map(|s| {
                            let p = s.path();
                            if p.extension().map(|e| e == "jsonl").unwrap_or(false) && !p.is_dir()
                            {
                                Some(p)
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let session_count = session_files
                .iter()
                .filter(|f| {
                    // 与 `sessions` 口径对齐：跳过 0 消息的空壳文件
                    if let Ok(events) = read_events(f) {
                        events
                            .iter()
                            .any(|ev| ev.get("type").and_then(|t| t.as_str()) == Some("message"))
                    } else {
                        false
                    }
                })
                .count() as i64;
            if session_count == 0 {
                continue;
            }

            // 反推项目路径：优先取任意事件里的 `cwd` 字段（权威来源），
            // 退而求其次用目录名机械替换（`-` → `/`，补前导 `/`）。
            let mut project_path = proj_name.replace('-', "/");
            if !project_path.starts_with('/') {
                project_path = format!("/{project_path}");
            }
            if let Some(file) = session_files.first()
                && let Ok(events) = read_events(file)
                && let Some(cwd) = events.iter().find_map(|ev| ev.get("cwd").and_then(|c| c.as_str()))
            {
                project_path = cwd.to_string();
            }

            let time_active = session_files
                .iter()
                .filter_map(|f| fs::metadata(f).and_then(|m| m.modified()).ok())
                .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).ok())
                .max();

            out.push(ProjectMeta {
                app: APP_NAME.to_string(),
                id: proj_name.clone(),
                path: project_path,
                vcs: None,
                session_count,
                subagent_count: 0,
                archived_count: 0,
                time_active,
            });
        }

        out.sort_by(|a, b| {
            b.time_active
                .unwrap_or(0)
                .partial_cmp(&a.time_active.unwrap_or(0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }

    fn project_paths_like(&self, pattern: &str) -> Result<Vec<String>> {
        let projects = self.projects()?;
        let needle = pattern.to_lowercase();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for p in &projects {
            if p.path.to_lowercase().contains(&needle) && seen.insert(p.path.clone()) {
                out.push(p.path.clone());
            }
        }
        Ok(out)
    }

    fn directories(&self) -> Result<Vec<DirectoryMeta>> {
        let files = self.list_session_files()?;
        let mut by_dir: HashMap<String, (i64, i64)> = HashMap::new();
        for file in &files {
            let Ok(events) = read_events(file) else {
                continue;
            };
            let Some(cwd) = events.iter().find_map(|ev| ev.get("cwd").and_then(|c| c.as_str()))
            else {
                continue;
            };
            let time_updated = events
                .iter()
                .filter_map(|ev| ev.get("timestamp").and_then(|t| t.as_i64()))
                .max()
                .unwrap_or(0);
            let entry = by_dir.entry(cwd.to_string()).or_insert((0, 0));
            entry.0 += 1;
            entry.1 = time_updated.max(entry.1);
        }

        let mut out: Vec<DirectoryMeta> = by_dir
            .into_iter()
            .map(|(path, (n, updated))| DirectoryMeta {
                path,
                session_count: n,
                time_updated: updated,
                project_count: 1,
            })
            .collect();
        out.sort_by_key(|d| std::cmp::Reverse(d.time_updated));
        Ok(out)
    }

    fn sessions(&self, filter: &SessionFilter, sort: SortKey) -> Result<Vec<SessionMeta>> {
        let files = self.list_session_files()?;

        // 项目过滤：从 `filter.project` 提取要匹配的路径列表。
        let filter_paths: Option<Vec<String>> = match &filter.project {
            ProjectSelector::All => None,
            ProjectSelector::Exact { path } => Some(vec![path.clone()]),
            ProjectSelector::Like { paths, .. } if paths.is_empty() => {
                return Ok(Vec::new())
            }
            ProjectSelector::Like { paths, .. } => Some(paths.clone()),
        };

        let mut out: Vec<SessionMeta> = Vec::new();

        for file in &files {
            let events = match read_events(file) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // `cwd` 是项目过滤和 `project_path` 的权威来源。
            let project_id = file
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let cwd: &str = events
                .iter()
                .find_map(|ev| ev.get("cwd").and_then(|c| c.as_str()))
                .unwrap_or("");

            if let Some(ref paths) = filter_paths {
                let matched = if !cwd.is_empty() {
                    paths.iter().any(|p| cwd == p || cwd.starts_with(&format!("{p}/")))
                } else {
                    // 没有 cwd 的早期空壳文件，用文件所在项目目录（编码名）
                    // 与过滤路径比较。
                    paths.iter().any(|p| {
                        let encoded = p
                            .trim_start_matches('/')
                            .replace('/', "-");
                        project_id == encoded
                    })
                };
                if !matched {
                    continue;
                }
            }

            let file_stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let Some(meta) = summarize(&events, &project_id, cwd, &file_stem) else {
                continue;
            };

            // 空壳过滤（无消息的会话）
            let has_messages = events
                .iter()
                .any(|ev| ev.get("type").and_then(|t| t.as_str()) == Some("message"));
            if !has_messages
                && filter.empty == aa_session_core::EmptySessions::Exclude
            {
                continue;
            }

            if let Some(dir) = &filter.directory
                && !meta.directory.contains(dir)
            {
                continue;
            }
            if let Some(title) = &filter.title_contains
                && !meta.title.to_lowercase().contains(&title.to_lowercase())
            {
                continue;
            }

            out.push(meta);
        }

        apply_sort(&mut out, sort);
        out.truncate(filter.limit);
        Ok(out)
    }

    fn session(&self, session_id: &str) -> Result<Option<SessionMeta>> {
        // session_id 在 JSONL 里是 `sessionId` 字段，也等于文件名（去掉 .jsonl）。
        // 先按文件名精确匹配，再扫所有文件找 sessionId。
        let projects_dir = self.projects_dir();
        if !projects_dir.exists() {
            return Ok(None);
        }

        let proj_entries = match fs::read_dir(&projects_dir) {
            Ok(entries) => entries.flatten().collect::<Vec<_>>(),
            Err(_) => return Ok(None),
        };
        for proj in &proj_entries {
            let proj_dir = proj.path();
            if !proj_dir.is_dir() {
                continue;
            }
            let candidate = proj_dir.join(format!("{session_id}.jsonl"));
            if candidate.exists()
                && let Ok(events) = read_events(&candidate)
            {
                let project_id = proj.file_name().to_string_lossy().to_string();
                // project_path 取 cwd（权威来源），退而用编码目录名。
                let project_path = events
                    .iter()
                    .find_map(|ev| ev.get("cwd").and_then(|c| c.as_str()))
                    .map(str::to_string)
                    .unwrap_or(project_id.clone());
                let file_stem = session_id.to_string();
                if let Some(meta) = summarize(&events, &project_id, &project_path, &file_stem) {
                    return Ok(Some(meta));
                }
            }
        }

        Ok(None)
    }

    /// 导出该会话的**全部原始数据**（无损）。
    ///
    /// 读该会话的整个 JSONL 文件，每行事件原样进 `events` 数组，
    /// 不解析、不挑事件类型。返回结构：
    /// ```json
    /// { "file": "<uuid>.jsonl", "events": [ ... 全部行 ... ] }
    /// ```
    fn dump(&self, session_id: &str) -> Result<serde_json::Value> {
        let file = self.find_session_file(session_id)?;
        let events = read_events(&file)?;
        let file_name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(serde_json::json!({
            "file": file_name,
            "events": events,
        }))
    }


}

/// 应用排序（简化版，不区分 tokens）。
fn apply_sort(out: &mut [SessionMeta], sort: SortKey) {
    match sort {
        SortKey::UpdatedDesc => {
            out.sort_by(|a, b| b.time_updated.cmp(&a.time_updated).then(a.id.cmp(&b.id)))
        }
        SortKey::CreatedDesc => {
            out.sort_by(|a, b| b.time_created.cmp(&a.time_created).then(a.id.cmp(&b.id)))
        }
        SortKey::Title => {
            out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then(a.id.cmp(&b.id)))
        }
        SortKey::Tokens => {
            out.sort_by(|a, b| {
                let ta = a.tokens_input + a.tokens_output;
                let tb = b.tokens_input + b.tokens_output;
                tb.cmp(&ta).then(a.id.cmp(&b.id))
            })
        }
    }
}
