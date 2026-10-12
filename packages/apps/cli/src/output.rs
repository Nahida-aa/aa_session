//! 终端输出：表格 / JSON。
//!
//! 排版细节集中在这里，让 `main.rs` 只管流程编排。
//!
//! 宽度按「中文算 2 列」估算（`str_width`），否则中英文混排的表格会错位。

/// 安全打印到 stdout：写失败（含管道被关闭）时静默忽略。
///
/// 不能用 `println!`：它对写错误会 `panic!`，于是
/// `aa-session ... | head -5` 这种正常用法会炸出
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

use crate::format::Format;
use crate::table;

/// 路径列的显示宽度上限（显示列宽，非字符数）。
const PATH_W: usize = 52;
use anyhow::Result;

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
pub fn print_projects(rows: &[ProjectMeta], fmt: Format) -> Result<()> {
    if let Some(rendered) = fmt.render(rows)? {
        out(format_args!("{rendered}"));
        return Ok(());
    }

    if rows.is_empty() {
        out(format_args!("没有找到项目"));
        return Ok(());
    }

    let mut t = table::build(&["会话", "子会话", "归档", "最近活跃", "项目路径"]);

    for p in rows {
        // opencode 用字面量 "/" 表示「无法确定所属仓库」，显示成「未知」，
        // 否则用户会以为有个项目挂在根目录上。
        let path = if p.is_unknown_path() {
            "（未知）".to_string()
        } else if p.path.is_empty() {
            "(无目录)".to_string()
        } else {
            // 路径从左侧截断：尾部才是区分信息。
            table::ellipsize_left(&p.path, PATH_W)
        };

        t.add_row(vec![
            table::cell(p.session_count),
            table::cell(p.subagent_count),
            table::cell(p.archived_count),
            table::cell(fmt_time(p.time_active.unwrap_or(0))),
            table::cell(path),
        ]);
    }

    out(format_args!("{}", table::render(t)));
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
pub fn print_directories(rows: &[DirectoryMeta], fmt: Format) -> Result<()> {
    if let Some(rendered) = fmt.render(rows)? {
        out(format_args!("{rendered}"));
        return Ok(());
    }

    if rows.is_empty() {
        out(format_args!("没有匹配的会话目录"));
        return Ok(());
    }

    let mut t = table::build(&["会话", "项目数", "最近活动", "目录"]);
    table::right_align(&mut t, &[0, 1]);

    for d in rows {
        t.add_row(vec![
            table::cell(d.session_count),
            table::cell(d.project_count),
            table::cell(fmt_time(d.time_updated)),
            table::cell(table::ellipsize_left(&d.path, PATH_W)),
        ]);
    }

    out(format_args!("{}", table::render(t)));
    out(format_args!("\n共 {} 个目录", rows.len()));
    Ok(())
}

/// 打印会话列表。
pub fn print_sessions(rows: &[SessionMeta], fmt: Format) -> Result<()> {
    if let Some(rendered) = fmt.render(rows)? {
        out(format_args!("{rendered}"));
        return Ok(());
    }

    if rows.is_empty() {
        out(format_args!("没有匹配的会话"));
        return Ok(());
    }

    // 每条会话一行，路径各自独立成列 —— 之前把路径塞进附注行再折行，
    // 结果一条会话占 2-3 行，几十行就滚过去了。
    let mut t = table::build(&[
        "会话 ID",
        "时间",
        "tokens",
        "agent",
        "成本",
        "标题",
        "会话目录",
        "项目目录",
    ]);
    table::right_align(&mut t, &[2, 4]);

    for r in rows {
        let title = table::ellipsize_right(&r.title.replace('\n', " "), TITLE_W);

        // 会话目录 = session_v2.directory（运行目录），总是显示。
        let session_dir = table::ellipsize_left(&r.directory, PATH_W);

        // 项目目录 = project.worktree（git 仓库根）。与会话目录相同时只写「同左」，
        // **不能留空** —— 空单元格看起来像「数据缺失」，而不是「和左边一样」。
        let project_dir = if r.is_unknown_project() {
            "（未知）".to_string()
        } else if r.project_path == r.directory {
            "同左".to_string()
        } else {
            table::ellipsize_left(&r.project_path, PATH_W)
        };

        let mut marks = String::new();
        if r.parent_id.is_some() {
            marks.push('↳');
        }
        if r.is_archived() {
            marks.push('⊘');
        }

        t.add_row(vec![
            table::cell(marks + &table::short_id(&r.id)),
            table::cell(fmt_time(r.time_updated)),
            table::cell(human_tokens(r.tokens_input + r.tokens_output)),
            table::cell(r.agent.as_deref().unwrap_or("-")),
            table::cell(cost_str(r.cost)),
            table::cell(if title.is_empty() {
                "(无标题)"
            } else {
                &title
            }),
            table::cell(session_dir),
            table::cell(project_dir),
        ]);
    }

    out(format_args!("{}", table::render(t)));
    out(format_args!(
        "\n共 {} 条会话（↳ 子会话，⊘ 已归档）",
        rows.len()
    ));
    Ok(())
}

/// 标题列的宽度上限。
const TITLE_W: usize = 40;

/// 打印登录态。
///
/// `table` 格式走**通用键值树**打印：登录态是个嵌套对象而不是行集合，
/// 硬套表格要么丢字段要么每列都空着一大半。树形打印对任何 app 的结构都成立，
/// CLI 因此不含 per-app 的字段知识。
/// `json` / `toon` 直接序列化整个对象（字段最全，给脚本和 AI 用）。
pub fn print_auth(
    info: &serde_json::Value,
    fmt: Format,
    show_secrets: bool,
) -> Result<()> {
    if let Some(rendered) = fmt.render(info)? {
        out(format_args!("{rendered}"));
        return Ok(());
    }

    if show_secrets {
        // 明码凭据上屏，先在屏幕和 stderr 各留一句警告。
        // 用户可能正把 stdout 重定向进文件或管道给 AI，那句提醒更值钱。
        eprintln!("⚠ 正在打印完整凭据（token / api key）。不要把它粘到 issue、日志或聊天里。");
        eprintln!("⚠ 凭据原文在 {} 里，日常查看不需要 --show-secrets。", credential_dir_hint(info));
    }

    let app = info.get("app").and_then(|a| a.as_str()).unwrap_or("?");
    let status = info.get("status").and_then(|s| s.as_str()).unwrap_or("?");

    let status_cn = match status {
        "logged_in" => "已登录",
        "not_logged_in" => "未登录",
        "unreadable" => "凭据无法解析",
        _ => status,
    };
    out(format_args!("{app} 登录状态：{status_cn}"));

    print_tree(info, 1);
    Ok(())
}

/// 从 auth 结果里取凭据文件路径，用于警告文案；取不到返回占位。
fn credential_dir_hint(info: &serde_json::Value) -> String {
    info.get("credential_dir")
        .or_else(|| info.get("auth_file"))
        .and_then(|v| v.as_str())
        .unwrap_or("凭据文件")
        .to_string()
}

/// 递归打印 JSON 值；超过该深度的容器折叠成一行摘要。
const MAX_DEPTH: usize = 6;
/// 数组/对象超过这个元素个数就只打印个数（登录态里有几十个轮转凭据）。
const MAX_ITEMS: usize = 12;

fn print_tree(value: &serde_json::Value, depth: usize) {
    use serde_json::Value;
    let pad = "  ".repeat(depth);

    match value {
        Value::Object(map) => {
            let keys: Vec<&String> = map.keys().collect();
            for k in keys {
                // app / status 已由表头说明，树里再重复一遍显得啰嗦。
                if depth == 1 && (k == "app" || k == "status") {
                    continue;
                }
                let v = &map[k];
                let shown = preview(v);
                out(format_args!("{pad}{k}：{shown}"));
                if needs_recurse(v, depth) {
                    print_tree(v, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            let shown = items.iter().take(MAX_ITEMS).count();
            for (i, v) in items.iter().enumerate() {
                if i >= MAX_ITEMS {
                    let rest = items.len() - MAX_ITEMS;
                    out(format_args!("{pad}…还有 {rest} 项"));
                    break;
                }
                out(format_args!("{pad}[{i}] {}", preview(v)));
                if needs_recurse(v, depth) {
                    print_tree(v, depth + 1);
                }
            }
            let _ = shown;
        }
        other => out(format_args!("{pad}{}", preview(other))),
    }
}

/// 单行预览值：标量原样，容器显示「类型 + 规模」。
fn preview(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "-".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if s.is_empty() {
                "(空)".to_string()
            } else {
                table::ellipsize_right(&s.replace('\n', " "), 60)
            }
        }
        Value::Array(a) => format!("[{} 项]", a.len()),
        Value::Object(m) => format!("{{{}}} 个字段", m.len()),
    }
}

/// 该值是否值得展开下一层。
fn needs_recurse(v: &serde_json::Value, depth: usize) -> bool {
    use serde_json::Value;
    if depth >= MAX_DEPTH {
        return false;
    }
    match v {
        Value::Object(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    }
}

