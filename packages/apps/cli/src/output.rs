//! 终端输出：表格 / JSON。
//!
//! 排版细节集中在这里，让 `main.rs` 只管流程编排。
//!
//! 宽度按「中文算 2 列」估算（`str_width`），否则中英文混排的表格会错位。

/// 安全打印到 stdout：写失败（含管道被关闭）时静默忽略。
///
/// 不能用 `println!`：它对写错误会 `panic!`，于是
/// `aa_session ... | head -5` 这种正常用法会炸出
/// `failed printing to stdout: Broken pipe`。
///
/// 忽略写错误对 CLI 是正确行为 —— 管道下游（`head`、`less`）提前退出时，
/// 上游本就该安静地结束而不是崩溃。
pub(crate) fn out(args: std::fmt::Arguments<'_>) {
    // 手动补换行：`println!` 会加，`format_args!` 不会。
    use std::io::Write as _;
    let mut w = std::io::stdout().lock();
    let _ = w.write_fmt(args);
    let _ = w.write_all(b"\n");
}

use aa_session_core::{DirectoryMeta, ProjectMeta, SessionMeta};
use anyhow::Result;

/// 字符串的显示宽度（CJK 记 2 列）。
fn str_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// 人类可读的体积。
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// 按显示宽度截断，超出时以 `…` 结尾。
fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = if c.is_ascii() { 1 } else { 2 };
        if w + cw > max.saturating_sub(1) {
            out.push('…');
            return out;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// 按显示宽度左侧补空格。
fn pad(s: &str, width: usize) -> String {
    let w = str_width(s);
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

/// Unix 毫秒 → `YYYY-MM-DD HH:MM`。
fn fmt_time(ms: i64) -> String {
    if ms <= 0 {
        return "-".to_string();
    }
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);

    // Howard Hinnant 的 civil_from_days 算法，不为一个字符串引入 chrono。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        tod / 3600,
        (tod % 3600) / 60
    )
}

/// 人类可读的 token 数（万 / 亿）。
fn human_tokens(n: i64) -> String {
    const WAN: i64 = 10_000;
    const YI: i64 = 100_000_000;
    if n >= YI {
        format!("{:.1}亿", n as f64 / YI as f64)
    } else if n >= WAN {
        format!("{:.1}万", n as f64 / WAN as f64)
    } else {
        n.to_string()
    }
}

/// 打印项目列表。
pub fn print_projects(rows: &[ProjectMeta], as_json: bool) -> Result<()> {
    if as_json {
        out(format_args!("{}", serde_json::to_string_pretty(rows)?));
        return Ok(());
    }

    if rows.is_empty() {
        out(format_args!("没有找到项目"));
        return Ok(());
    }

    // 「会话」列只数顶层会话（与 `sessions` 默认行为一致），子会话数单列。
    // 不显示来源列 —— `--app` 参数已经说明了是哪个 app。
    out(format_args!(
        "{:>6}  {:>6}  {:<16}  项目路径",
        "会话", "子会话", "最近活跃"
    ));

    for p in rows {
        // opencode 用字面量 "/" 表示「无法确定所属仓库」，显示成「未知」，
        // 否则用户会以为有个项目挂在根目录上。
        let path = if p.is_unknown_path() {
            "（未知）"
        } else if p.path.is_empty() {
            "(无目录)"
        } else {
            &p.path
        };
        out(format_args!(
            "{:>6}  {:>6}  {:>6}  {:<16}  {}",
            p.session_count,
            p.subagent_count,
            p.archived_count,
            fmt_time(p.time_active.unwrap_or(0)),
            truncate(path, 70)
        ));
    }

    out(format_args!("\n共 {} 个项目", rows.len()));
    Ok(())
}

/// 成本显示。`None` 表示上游没记录（opencode 多数会话不填这个字段）。
fn cost_str(cost: Option<f64>) -> String {
    match cost {
        Some(c) if c >= 0.01 => format!("${c:.2}"),
        Some(c) => format!("${c:.3}"),
        None => "-".to_string(),
    }
}

/// 打印会话运行目录列表。
pub fn print_directories(rows: &[DirectoryMeta], as_json: bool) -> Result<()> {
    if as_json {
        out(format_args!("{}", serde_json::to_string_pretty(rows)?));
        return Ok(());
    }

    if rows.is_empty() {
        out(format_args!("没有匹配的会话目录"));
        return Ok(());
    }

    // 「项目数」列大于 1 说明有会话跑在了不属于本项目的位置 —— opencode 的
    // 已知行为，不是 bug，但对排查有用，所以显式列出来。
    out(format_args!(
        "{:>6}  {:>6}  {:<16}  目录",
        "会话", "项目数", "最近活动"
    ));

    for d in rows {
        out(format_args!(
            "{:>6}  {:>6}  {:<16}  {}",
            d.session_count,
            d.project_count,
            fmt_time(d.time_updated),
            truncate(&d.path, 60)
        ));
    }

    out(format_args!("\n共 {} 个目录", rows.len()));
    Ok(())
}

/// 打印会话列表。
pub fn print_sessions(rows: &[SessionMeta], as_json: bool) -> Result<()> {
    if as_json {
        out(format_args!("{}", serde_json::to_string_pretty(rows)?));
        return Ok(());
    }

    if rows.is_empty() {
        out(format_args!("没有匹配的会话"));
        return Ok(());
    }

    out(format_args!(
        "{}  {}  {}  {:>8}  标题  ·  目录",
        pad("时间", 16),
        pad("tokens", 8),
        pad("agent", 8),
        pad("成本", 8),
    ));

    for r in rows {
        let tokens = human_tokens(r.tokens_input + r.tokens_output);
        let title = truncate(&r.title.replace('\n', " "), 34);
        let dir = r.directory.replace('\n', " ");
        let mut label = if title.is_empty() { dir.clone() } else { title };
        if !dir.is_empty() && !label.contains(&dir) && label.len() + dir.len() < 60 {
            label.push_str("  ·  ");
            label.push_str(&truncate(&dir, 24));
        }

        out(format_args!(
            "{}  {}  {}  {:>8}  {}",
            pad(&fmt_time(r.time_updated), 16),
            pad(&tokens, 8),
            pad(r.agent.as_deref().unwrap_or("-"), 8),
            pad(&cost_str(r.cost), 8),
            label
        ));

        // 附注行：子会话标出父会话；归档的标出时间；再带上改动文件数和推理开销。
        let mut notes: Vec<String> = Vec::new();
        if let Some(parent) = &r.parent_id {
            notes.push(format!("子会话 ← {parent}"));
        }
        if r.is_archived() {
            notes.push(format!("已归档 {}", fmt_time(r.archived_at.unwrap_or(0))));
        }
        if r.files_changed > 0 {
            notes.push(format!("改动 {} 个文件", r.files_changed));
        }
        if r.tokens_reasoning > 0 {
            notes.push(format!("推理 {}", human_tokens(r.tokens_reasoning)));
        }

        out(format_args!("      {}  {}", r.id, notes.join("  ·  ")));
    }

    out(format_args!("\n共 {} 条会话", rows.len()));
    Ok(())
}
