//! 表格渲染。
//!
//! 用 [`comfy_table`] 而不是手写 `{:>6}` 格式化的原因：
//!
//! - **CJK 宽度对齐**。我们的路径全是中文，`{:>10}` 按字符数补空格，中文列必然
//!   错位。comfy-table 用 `unicode-width` 按显示列宽算。
//! - **结构完整**。表头下横线、右对齐数字列、内容不换行，都交给库处理。
//!
//! # 路径一律「左截断」
//!
//! `/home/aa/repos/env_ls/LocalDub` 与 `…/env_ls/LocalDub/apps/web` 有很长的
//! 共同前缀，**尾部才是区分信息**。所以从左边截断（`…/env_ls/LocalDub`），
//! 砍尾部会让所有行看起来一模一样。

use comfy_table::presets;
use comfy_table::{Cell, CellAlignment, ContentArrangement, Row, Table};
use unicode_width::UnicodeWidthStr;

/// 字符串的显示列宽。
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// 按显示列宽从**左侧**截断，保留尾部；超出部分以 `…` 开头。
pub fn ellipsize_left(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    // `…` 自己就占 1 列，放不下时连它都不能给 —— 否则输出会比预算还宽。
    if max == 0 {
        return String::new();
    }
    let budget = max.saturating_sub(1);

    let mut out = String::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        out.insert(0, c);
        w += cw;
    }
    format!("…{out}")
}

/// 按显示列宽从**右侧**截断，保留开头（标题用这个：开头信息量更大）。
pub fn ellipsize_right(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max.saturating_sub(1);

    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        out.push(c);
        w += cw;
    }
    format!("{out}…")
}

/// 建一张表：只保留表头下的横线，不画外框和竖线。
///
/// 全框在 891 行时会画出 891 个框，视觉噪声压过信息。
pub fn build(headers: &[&str]) -> Table {
    let mut table = Table::new();
    table.load_style(presets::NOTHING);
    table.set_header(Row::from(headers));
    // 路径是主要信息，不希望在半宽时被折行打断。
    table.set_content_arrangement(ContentArrangement::Disabled);
    table
}

/// 把表渲染成字符串；无行时返回空串。
pub fn render(table: Table) -> String {
    table.to_string()
}

/// 造一个普通单元格。
pub fn cell(s: impl std::fmt::Display) -> Cell {
    Cell::new(s.to_string())
}

/// 会话 id 的显示形式：固定前 12 位 + 省略号。
///
/// opencode 的 id 形如 `ses_0123456789abcdef`，前 12 位足够定位，
/// 全量打印会把表格撑宽 28 列。
pub fn short_id(id: &str) -> String {
    const PREFIX: usize = 12;
    if id.len() <= PREFIX + 2 {
        id.to_string()
    } else {
        format!("{}…", &id[..PREFIX])
    }
}

/// 把指定列设为右对齐（数字列读起来更顺）。
pub fn right_align(t: &mut Table, columns: &[usize]) {
    for &c in columns {
        if let Some(col) = t.column_mut(c) {
            col.set_cell_alignment(CellAlignment::Right);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_cjk_as_two_columns() {
        // 这是手写 {:>N} 格式化会踩的坑：Rust 的 str::len() 按字节算，
        // "重构" 是 6 字节但只占 4 个显示列。
        assert_eq!(width("abc"), 3);
        assert_eq!(width("重构"), 4);
        assert_eq!(width("a重构b"), 6);
    }

    #[test]
    fn ellipsize_left_keeps_the_tail() {
        // 路径的尾部才是区分信息，前缀大家都一样。
        let long = "/home/aa/repos/env_ls/LocalDub";
        let got = ellipsize_left(long, 16);
        assert_eq!(width(&got), 16, "截断后宽度应正好等于上限");
        assert!(got.starts_with('…'), "应保留尾部、前面加省略号");
        assert!(got.ends_with("LocalDub"), "尾部必须保留：{got}");
    }

    #[test]
    fn ellipsize_left_respects_cjk_width() {
        let got = ellipsize_left("/路径/很长/很长/很长", 10);
        assert!(width(&got) <= 10, "实际 {} 列", width(&got));
    }

    #[test]
    fn ellipsize_left_noop_when_short_enough() {
        assert_eq!(ellipsize_left("/a/b", 20), "/a/b");
        // 正好等于上限也不该动
        assert_eq!(ellipsize_left("/a/b", 4), "/a/b");
    }

    #[test]
    fn ellipsize_right_keeps_the_head() {
        // 标题相反：开头信息量更大。
        let got = ellipsize_right("New session - 2026-10-02T06:40:12.792Z", 20);
        assert_eq!(width(&got), 20);
        assert!(got.starts_with("New session"), "{got}");
        assert!(got.ends_with('…'));
    }

    #[test]
    fn ellipsize_handles_zero_budget_without_panic() {
        assert_eq!(ellipsize_left("abc", 0), "");
        assert_eq!(ellipsize_right("abc", 0), "");
    }

    #[test]
    fn short_id_keeps_identifying_prefix() {
        assert_eq!(short_id("ses_0123456789abcdef"), "ses_01234567…");
        assert_eq!(short_id("short"), "short");
        // 边界：正好不超就不该被截
        assert_eq!(short_id("ses_012345"), "ses_012345");
    }
}
