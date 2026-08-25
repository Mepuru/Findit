//! 分类 API（转发到 `core::repo::categories`）。

use crate::api::model::Category;
use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::repo;

pub async fn create_category(name: String) -> Result<Category, FinditError> {
    with_conn(|conn| repo::categories::create_category(conn, &name))
}

pub async fn list_categories() -> Result<Vec<Category>, FinditError> {
    with_conn(repo::categories::list_categories)
}

pub async fn rename_category(id: i64, new_name: String) -> Result<Category, FinditError> {
    with_conn(|conn| repo::categories::rename_category(conn, id, &new_name))
}

pub async fn delete_category(id: i64) -> Result<bool, FinditError> {
    with_conn(|conn| repo::categories::delete_category(conn, id).map(|_| true))
}
