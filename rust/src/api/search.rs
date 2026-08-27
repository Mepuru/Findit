//! 搜索 API（转发到 `core::search`）。

use crate::api::model::SearchResult;
use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::search;

/// 双通道搜索。
///
/// `embedding` 为查询向量（由调用方生成）；`None` 时只做关键词搜索。
/// 语义命中在前并附带相似度百分比，关键词命中在后。
///
/// P-H3 三阶段编排，把最重的向量打分移出全局数据库锁：
/// 1. 短锁：取进程内向量索引（缓存命中时零数据库读取）；
/// 2. 锁外：纯内存 top-K 余弦打分；
/// 3. 锁内：关键词搜索 + 语义记录回查 + 合并去重。
pub async fn search_items(
    query: String,
    embedding: Option<Vec<f32>>,
) -> Result<Vec<SearchResult>, FinditError> {
    // 阶段 1（短锁）：向量索引（缓存未命中才读库重建）。
    let index = with_conn(|conn| search::semantic::cached_embedding_index(conn))?;

    // 阶段 2（锁外）：纯内存打分，不触碰数据库。
    let scored: Vec<(i64, f32)> = match embedding.as_deref() {
        Some(v) => search::semantic::score_top_k(&index, v, search::semantic::SEMANTIC_TOP_K),
        None => Vec::new(),
    };

    // 阶段 3（锁内）：关键词 + 语义记录加载与合并。
    with_conn(|conn| search::hybrid::search_with_index(conn, &query, &scored))
}
