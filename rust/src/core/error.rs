//! 统一业务错误类型。
//!
//! 该枚举同时服务于两处：
//! - `core` 层作为业务错误返回值；
//! - `api` 层作为 `Result<T, FinditError>` 的 `Err`，由
//!   flutter_rust_bridge 镜像为 Dart 端可区分的异常类型。
//!
//! 为了让 FRB 能够把该枚举镜像为 Dart 类，所有变体字段均使用
//! FRB 友好的基本类型（String / i64）。

use thiserror::Error;

/// Findit 业务错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FinditError {
    /// 数据库尚未通过 `init_db` 初始化。
    #[error("数据库尚未初始化，请先调用 init_db")]
    DbNotInitialized,

    /// 底层 SQLite / rusqlite 错误。
    #[error("数据库错误：{0}")]
    Db(String),

    /// 唯一约束冲突（重名）。
    /// `entity` 表示实体种类（如 "存储单元" / "收纳箱" / "分类"）。
    #[error("{entity}名称已存在：{name}")]
    DuplicateName { entity: String, name: String },

    /// 目标实体不存在。
    #[error("未找到{entity}：{hint}")]
    NotFound { entity: String, hint: String },

    /// 输入校验失败。
    #[error("{0}")]
    Validation(String),

    /// 文件 / 目录 IO 错误。
    #[error("IO 错误：{0}")]
    Io(String),
}

impl From<rusqlite::Error> for FinditError {
    fn from(e: rusqlite::Error) -> Self {
        FinditError::Db(e.to_string())
    }
}

impl From<std::io::Error> for FinditError {
    fn from(e: std::io::Error) -> Self {
        FinditError::Io(e.to_string())
    }
}

/// 便捷别名。
pub type FinditResult<T> = Result<T, FinditError>;
