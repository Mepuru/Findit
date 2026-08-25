//! 搜索 API（转发到 `core::search::hybrid`）。

use crate::api::model::SearchResult;
use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::search;

/// 双通道搜索。
///
/// `embedding` 为查询向量（由调用方生成）；`None` 时只做关键词搜索。
/// 语义命中在前并附带相似度百分比，关键词命中在后。
pub async fn search_items(
    query: String,
    embedding: Option<Vec<f32>>,
) -> Result<Vec<SearchResult>, FinditError> {
    with_conn(|conn| search::hybrid::search(conn, &query, embedding.as_deref()))
}
