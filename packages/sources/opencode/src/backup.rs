//! 写 opencode 库前的备份。
//!
//! # 为什么用 SQLite 的 `VACUUM INTO` 而不是文件复制
//!
//! 直接 `cp opencode.db` 是**错的**：库是 WAL 模式的，`-wal` 里可能还有未
//! checkpoint 的事务。只复制主文件会得到一个**残缺**的快照。
//!
//! - `VACUUM INTO 'path'` —— SQLite 自己做的在线一致性快照，包含 WAL 内容，
//!   产出的是一个干净的、独立的、可正常打开的数据库；
//! - `VACUUM INTO` 要求连接有写权限，所以要用读写连接打开（备份本身**不改**
//!   任何业务数据，只是产出新文件）。
//!
//! 备份文件是**完整库**（约 6 GB）。这是有意的：宁可占磁盘，也不能在需要时
//! 发现备份不完整。

use aa_session_core::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// 备份文件名后缀。
const BACKUP_SUFFIX: &str = ".aa_session_backup.sqlite";

/// 为数据库生成一个备份文件路径（同一目录，便于定位）。
pub fn backup_path_for(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    db.with_file_name(name)
}

/// 把 `db` 备份到 `dest`。
///
/// 已存在时**拒绝覆盖** —— 备份文件被静默覆盖等于没有备份。要重做请手动删。
pub fn backup(db: &Connection, dest: &Path) -> Result<PathBuf> {
    if dest.exists() {
        return Err(aa_session_core::Error::InvalidArgument(format!(
            "备份文件已存在：{}（不会覆盖，请手动删除或换一个路径）",
            dest.display()
        )));
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| aa_session_core::Error::Io {
            path: parent.display().to_string(),
            cause: e.to_string(),
        })?;
    }

    // VACUUM INTO 的目标不能是绑定参数（SQLite 语法限制），只能拼进 SQL，
    // 所以这里对单引号做转义。路径来自我们自己生成的文件名，不来自用户输入。
    let sql = format!(
        "VACUUM INTO '{}'",
        dest.display().to_string().replace('\'', "''")
    );
    db.execute_batch(&sql)
        .map_err(|e| aa_session_core::Error::Io {
            path: dest.display().to_string(),
            cause: format!("VACUUM INTO 失败：{e}"),
        })?;

    Ok(dest.to_path_buf())
}
