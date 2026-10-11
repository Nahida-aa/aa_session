use thiserror::Error;

/// 错误类型。
///
/// 库层一律用本类型，只有二进制入口（`main`）才用 `anyhow` 堆上下文
/// —— 这样库可以被 GUI 直接复用而不必传递 `anyhow::Error`。
#[derive(Debug, Error)]
pub enum Error {
    /// 打不开底层数据源（数据库文件 / 目录）。
    #[error("cannot open data source at {path}: {cause}")]
    Open {
        /// 数据源路径。
        path: String,
        /// 底层错误描述。
        cause: String,
    },

    /// 数据源查询失败。
    #[error("query failed: {0}")]
    Query(String),

    /// `data` 列里的 JSON 解析失败。
    ///
    /// 单条坏数据不应中断整体流程，因此 source 层会跳过并计入
    /// [`crate::MessageBatch::skipped`]。
    #[error("malformed json at {location}: {cause}")]
    MalformedJson {
        /// 出问题的位置（通常是 `message:<id>`）。
        location: String,
        /// `serde_json` 错误。
        cause: serde_json::Error,
    },

    /// 请求的对象不存在。
    #[error("not found: {0}")]
    NotFound(String),

    /// 调用方给出了非法参数。
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// 输出文件相关错误。
    #[error("io error at {path}: {cause}")]
    Io {
        /// 文件路径。
        path: String,
        /// 底层错误描述。
        cause: String,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
