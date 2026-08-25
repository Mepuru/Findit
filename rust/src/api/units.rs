//! 存储单元 API（转发到 `core::repo::units`）。

use crate::api::model::Unit;
use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::repo;

pub async fn create_unit(name: String, description: String) -> Result<Unit, FinditError> {
    with_conn(|conn| repo::units::create_unit(conn, &name, &description))
}

pub async fn list_units() -> Result<Vec<Unit>, FinditError> {
    with_conn(repo::units::list_units)
}

pub async fn get_unit(id: i64) -> Result<Unit, FinditError> {
    with_conn(|conn| repo::units::get_unit(conn, id))
}

pub async fn update_unit(
    id: i64,
    name: Option<String>,
    description: Option<String>,
) -> Result<Unit, FinditError> {
    with_conn(|conn| repo::units::update_unit(conn, id, name, description))
}

pub async fn delete_unit(id: i64) -> Result<bool, FinditError> {
    with_conn(|conn| repo::units::delete_unit(conn, id).map(|_| true))
}
