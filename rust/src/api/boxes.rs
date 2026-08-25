//! 收纳箱 API（转发到 `core::repo::boxes`）。

use crate::api::model::StorageBox;
use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::repo;

pub async fn create_box(
    unit_id: i64,
    name: String,
    description: String,
) -> Result<StorageBox, FinditError> {
    with_conn(|conn| repo::boxes::create_box(conn, unit_id, &name, &description))
}

pub async fn list_boxes(unit_id: i64) -> Result<Vec<StorageBox>, FinditError> {
    with_conn(|conn| repo::boxes::list_boxes(conn, unit_id))
}

pub async fn get_box(id: i64) -> Result<StorageBox, FinditError> {
    with_conn(|conn| repo::boxes::get_box(conn, id))
}

pub async fn get_box_by_slug(slug: String) -> Result<StorageBox, FinditError> {
    with_conn(|conn| repo::boxes::get_box_by_slug(conn, &slug))
}

pub async fn update_box(
    id: i64,
    name: Option<String>,
    description: Option<String>,
    unit_id: Option<i64>,
) -> Result<StorageBox, FinditError> {
    with_conn(|conn| repo::boxes::update_box(conn, id, name, description, unit_id))
}

pub async fn delete_box(id: i64) -> Result<bool, FinditError> {
    with_conn(|conn| repo::boxes::delete_box(conn, id).map(|_| true))
}
