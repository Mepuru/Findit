//! 应用设置 API（转发到 `core::repo::settings`）。

use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::repo;

/// 读取设置项；不存在返回 `null`。
pub async fn get_setting(key: String) -> Result<Option<String>, FinditError> {
    with_conn(|conn| repo::settings::get_setting(conn, &key))
}

/// 写入设置项（存在则覆盖）。返回 `true` 表示成功（FRB 异步接口不支持空返回）。
pub async fn set_setting(key: String, value: String) -> Result<bool, FinditError> {
    with_conn(|conn| repo::settings::set_setting(conn, &key, &value).map(|_| true))
}
