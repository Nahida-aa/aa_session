//! 输出格式。
//!
//! # 为什么表格和 TOON 都留
//!
//! 这两种格式**不是二选一**，它们解决的是不同的问题：
//!
//! | 格式 | 解决什么 | 谁看 |
//! |---|---|---|
//! | `table`（默认） | **视觉对齐**，纵向扫一列对比数字时最省力 | 人眼 |
//! | `toon` | **省 token**：表头写一次键名，行内只写值 | AI / token 敏感场景 |
//! | `json` | 通用管道（`jq`）、生态兼容 | 脚本、既有工作流 |
//!
//! TOON 的输出人也能读，但它的列**不对齐**，所以替代不了表格。
//! 反过来表格对 AI 极不友好——数字带千分位和前后空格。
//!
//! 实测：5 条会话 TOON 比 JSON 省 **34%**（省的是重复的键名；字段越长越散，
//! 比如路径，省的越少）。
//!
//! `--json` 保留为 `--format json` 的简写，避免打断既有调用。

use anyhow::Result;
use serde::Serialize;

/// 给 `toon_format::encode_default` 用的 Sized 包装（它要求 `T: Sized`）。
struct SizedSlice<'a, T: ?Sized>(&'a T);

impl<T: Serialize + ?Sized> Serialize for SizedSlice<'_, T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

/// 列表类输出的格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// 对齐表格，给人扫。
    #[default]
    Table,
    /// JSON，通用管道。
    Json,
    /// TOON，省 token，人也能读。
    Toon,
}

impl Format {
    /// 把值序列化成该格式；`Table` 返回 `None`（表格不走序列化路径）。
    pub fn render<T: Serialize + ?Sized>(self, value: &T) -> Result<Option<String>> {
        match self {
            Format::Table => Ok(None),
            Format::Json => Ok(Some(serde_json::to_string_pretty(value)?)),
            // `encode_default<T: Serialize>` 要求 `Sized`，切片给不了。
            // 用一个 Sized 的 newtype 包住切片，而不是把调用点改成 `&Vec<T>`
            // （`own-02` 那条规则就是为这种签名妥协准备的）。
            Format::Toon => Ok(Some(toon_format::encode_default(&SizedSlice(value))?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 往返测试需要 Deserialize；只 derive Serialize 编译不过。
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Row {
        id: String,
        n: i64,
    }

    fn rows() -> Vec<Row> {
        vec![
            Row {
                id: "a".into(),
                n: 1,
            },
            Row {
                id: "b".into(),
                n: 2,
            },
        ]
    }

    #[test]
    fn table_is_not_serialized() {
        assert!(Format::Table.render(&rows()).unwrap().is_none());
    }

    #[test]
    fn json_round_trips() {
        let s = Format::Json.render(&rows()).unwrap().unwrap();
        let back: Vec<Row> = serde_json::from_str(&s).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].id, "a");
    }

    #[test]
    fn toon_is_compact_and_has_a_header() {
        let json = Format::Json.render(&rows()).unwrap().unwrap();
        let toon = Format::Toon.render(&rows()).unwrap().unwrap();

        // 表头声明行数与键名 —— 这是 TOON 省 token 的地方
        assert!(toon.starts_with("[2]{"), "实际: {toon}");
        assert!(toon.contains("id"));
        assert!(toon.len() < json.len(), "TOON 应比 JSON 小");
    }

    #[test]
    fn both_machine_formats_parse_back_into_the_same_data() {
        let orig = rows();
        let json: Vec<Row> =
            serde_json::from_str(&Format::Json.render(&orig).unwrap().unwrap()).unwrap();
        let toon: Vec<Row> =
            toon_format::decode_default(&Format::Toon.render(&orig).unwrap().unwrap()).unwrap();

        assert_eq!(json.len(), toon.len());
        for (a, b) in json.iter().zip(toon.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.n, b.n);
        }
    }
}
