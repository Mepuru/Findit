//! 数据库生命周期。

use crate::core::error::FinditError;

/// 打开（或创建）数据库并应用迁移。`db_dir` 为存放 `findit.db` 的目录。
/// 由 Dart 侧在 `RustLib.init()` 之后、调用其他 API 之前调用。
/// 返回 `true` 表示成功（FRB 异步接口不支持空返回，用 bool 占位）。
pub async fn init_db(db_dir: String) -> Result<bool, FinditError> {
    crate::core::db::init_db(&db_dir).map(|_| true)
}
