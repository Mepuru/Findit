//! 物品 API（转发到 `core::repo::items`）。

use crate::api::model::Item;
use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::repo;

pub async fn create_item(
    box_id: i64,
    name: String,
    description: String,
    quantity: i64,
    category_ids: Vec<i64>,
) -> Result<Item, FinditError> {
    with_conn(|conn| repo::items::create_item(conn, box_id, &name, &description, quantity, &category_ids))
}

pub async fn list_items(box_id: i64) -> Result<Vec<Item>, FinditError> {
    with_conn(|conn| repo::items::list_items(conn, box_id))
}

pub async fn get_item(id: i64) -> Result<Item, FinditError> {
    with_conn(|conn| repo::items::get_item(conn, id))
}

pub async fn update_item(
    id: i64,
    name: Option<String>,
    description: Option<String>,
    quantity: Option<i64>,
    box_id: Option<i64>,
    category_ids: Option<Vec<i64>>,
) -> Result<Item, FinditError> {
    with_conn(|conn| repo::items::update_item(conn, id, name, description, quantity, box_id, category_ids))
}

pub async fn delete_item(id: i64) -> Result<bool, FinditError> {
    with_conn(|conn| repo::items::delete_item(conn, id).map(|_| true))
}
