//! 物品照片 API（转发到 `core::photo`）。

use crate::core::db::{photos_dir, with_conn};
use crate::core::error::FinditError;
use crate::core::photo;

/// 保存物品照片（P-H1 三阶段，压缩不持全局数据库锁）：
/// 1. 短锁：校验物品存在并取旧照片路径；
/// 2. 锁外：压缩落盘（解码 + 缩放 + 编码，可能耗时数百毫秒至秒级）；
/// 3. 短锁：登记 `photo_path` 并清理旧文件。
///
/// 拍照入库期间其它数据库操作（列表加载、搜索、录入）不再被阻塞。
/// 返回新主图文件名。
pub async fn save_item_photo(item_id: i64, bytes: Vec<u8>) -> Result<String, FinditError> {
    // 阶段 1（短锁）：仅一次主键查询。
    let old_photo = with_conn(|conn| photo::existing_item_photo(conn, item_id))?;

    // 阶段 2（锁外）：压缩落盘。
    let dir = photos_dir()?;
    let new_name = photo::save_photo_bytes(&bytes, &dir)?;

    // 阶段 3（短锁）：登记路径 + 清理旧文件。
    with_conn(|conn| photo::register_item_photo(conn, item_id, &new_name, old_photo.as_deref()))?;
    Ok(new_name)
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
