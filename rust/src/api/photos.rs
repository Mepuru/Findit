//! 物品照片 API（转发到 `core::photo`）。

use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::photo;

/// 保存物品照片：压缩（主图 1600px/q82 + 缩略图 256px/q70）落盘，
/// 更新 `items.photo_path` 并清理旧照片文件。返回新主图文件名。
pub async fn save_item_photo(item_id: i64, bytes: Vec<u8>) -> Result<String, FinditError> {
    with_conn(|conn| photo::save_item_photo(conn, item_id, &bytes))
}

/// 删除物品照片：清空 `photo_path` 并移除主图与缩略图文件。
pub async fn delete_item_photo(item_id: i64) -> Result<bool, FinditError> {
    with_conn(|conn| photo::delete_item_photo(conn, item_id).map(|_| true))
}

/// 主图文件的完整本地路径（入参为 `items.photo_path`）。
pub async fn get_photo_full_path(file_name: String) -> Result<String, FinditError> {
    photo::photo_full_path(&file_name)
}

/// 缩略图文件的完整本地路径（入参为主图文件名）。
pub async fn get_thumb_full_path(file_name: String) -> Result<String, FinditError> {
    photo::thumb_full_path(&file_name)
}
