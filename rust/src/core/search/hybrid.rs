//! 混合搜索：语义结果在前（按相似度降序、带百分比），关键词结果在后，
//! 同一物品去重时语义优先。
//!
//! 本阶段查询向量由调用方提供（`Option<&[f32]>`）；无向量时只做关键词搜索。

use std::collections::{HashMap, HashSet};

use rusqlite::{params_from_iter, Connection};

use crate::api::model::{Item, MatchedBy, SearchResult};
use crate::core::error::FinditResult;
use crate::core::repo::load_item_categories;
use crate::core::search::keyword;
use crate::core::search::semantic::{
    blob_to_embedding, cosine_similarity, passes_threshold, similarity_percent,
};

/// 主搜索入口。
///
/// - `query_embedding` 为 `Some` 时：对全部有 `embedding` 的物品计算相似度，
///   取 ≥ 阈值的按相似度降序作为语义结果（每条带 0-100 整数百分比）；
/// - 关键词命中作为第二部分，剔除已被语义命中的物品；
/// - 空查询且无向量时返回空列表。
pub fn search(
    conn: &Connection,
    query: &str,
    query_embedding: Option<&[f32]>,
) -> FinditResult<Vec<SearchResult>> {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut results: Vec<SearchResult> = Vec::new();

    if let Some(query_vec) = query_embedding {
        for record in semantic_search(conn, query_vec)? {
            seen.insert(record.0.id);
            results.push(SearchResult {
                id: record.0.id,
                name: record.0.name,
                description: record.0.description,
                quantity: record.0.quantity,
                photo_path: record.0.photo_path,
                box_id: record.0.box_id,
                categories: record.0.categories,
                box_name: record.1,
                unit_name: record.2,
                matched_by: MatchedBy::Semantic,
                similarity_percent: Some(record.3),
            });
        }
    }

    for hit in keyword::search_keyword(conn, query)? {
        if !seen.insert(hit.item.id) {
            continue;
        }
        results.push(SearchResult {
            id: hit.item.id,
            name: hit.item.name,
            description: hit.item.description,
            quantity: hit.item.quantity,
            photo_path: hit.item.photo_path,
            box_id: hit.item.box_id,
            categories: hit.item.categories,
            box_name: hit.box_name,
            unit_name: hit.unit_name,
            matched_by: MatchedBy::Keyword,
            similarity_percent: None,
        });
    }

    Ok(results)
}

/// 语义通道：相似度 ≥ 阈值的物品，按相似度降序，附带整数百分比。
fn semantic_search(
    conn: &Connection,
    query_vec: &[f32],
) -> FinditResult<Vec<(Item, String, String, i32)>> {
    let mut stmt = conn.prepare("SELECT id, embedding FROM items WHERE embedding IS NOT NULL")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // 相似度过滤 + 降序排序（同分按 id 稳定排序）。
    let mut scored: Vec<(i64, f32)> = Vec::new();
    for (id, blob) in rows {
        let Some(vec) = blob_to_embedding(&blob) else {
            continue; // BLOB 损坏：跳过。
        };
        if vec.len() != query_vec.len() {
            continue; // 维度不一致：无法比较。
        }
        let sim = cosine_similarity(query_vec, &vec);
        if passes_threshold(sim) {
            scored.push((id, sim));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });

    let ids: Vec<i64> = scored.iter().map(|(id, _)| *id).collect();
    let records = load_items_with_location(conn, &ids)?;

    Ok(scored
        .into_iter()
        .zip(records)
        .map(|((_, sim), record)| (record.0, record.1, record.2, similarity_percent(sim)))
        .collect())
}

/// 按 id 批量读取物品（含分类、所在箱/单元名），结果顺序与输入 `ids` 一致。
fn load_items_with_location(
    conn: &Connection,
    ids: &[i64],
) -> FinditResult<Vec<(Item, String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT i.id, i.name, i.description, i.quantity, i.photo_path, i.box_id, \
                COALESCE(i.created_at, ''), COALESCE(i.updated_at, ''), \
                b.name, u.name \
         FROM items i \
         JOIN storage_boxes b ON b.id = i.box_id \
         JOIN storage_units u ON u.id = b.unit_id \
         WHERE i.id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(ids.iter().copied()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let cats = load_item_categories(conn, ids)?;
    let mut by_id: HashMap<i64, (Item, String, String)> = HashMap::new();
    for (id, name, description, quantity, photo_path, box_id, created_at, updated_at, box_name, unit_name) in rows
    {
        let mut categories = cats.get(&id).cloned().unwrap_or_default();
        categories.sort();
        by_id.insert(
            id,
            (
                Item {
                    id,
                    name,
                    description,
                    quantity,
                    photo_path,
                    box_id,
                    categories,
                    created_at,
                    updated_at,
                },
                box_name,
                unit_name,
            ),
        );
    }

    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::{boxes, categories, items, units};
    use crate::core::search::semantic::embedding_to_blob;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn set_embedding(conn: &Connection, item_id: i64, vec: &[f32]) {
        conn.execute(
            "UPDATE items SET embedding = ?2 WHERE id = ?1",
            rusqlite::params![item_id, embedding_to_blob(vec)],
        )
        .unwrap();
    }

    /// 造数据：「扳手」（有向量，语义可命中）与「螺丝刀」（描述含“修水管的工具”）。
    fn fixture(conn: &Connection) -> (i64, i64) {
        let unit = units::create_unit(conn, "车库", "").unwrap();
        let box_ = boxes::create_box(conn, unit.id, "工具箱", "").unwrap();
        let tools = categories::create_category(conn, "五金").unwrap();
        let wrench = items::create_item(conn, box_.id, "扳手", "工业级", 1, &[tools.id]).unwrap();
        let driver = items::create_item(conn, box_.id, "螺丝刀", "修水管的工具", 1, &[]).unwrap();
        // 扳手向量与查询向量夹角余弦 ≈ 0.707 ≥ 0.3。
        set_embedding(conn, wrench.id, &[1.0, 1.0, 0.0]);
        (wrench.id, driver.id)
    }

    #[test]
    fn keyword_only_when_no_embedding() {
        let conn = setup();
        let (_wrench, _driver) = fixture(&conn);

        let results = search(&conn, "修水管", None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "螺丝刀");
        assert_eq!(results[0].matched_by, MatchedBy::Keyword);
        assert!(results[0].similarity_percent.is_none());
        assert_eq!(results[0].box_name, "工具箱");
        assert_eq!(results[0].unit_name, "车库");
    }

    #[test]
    fn semantic_results_come_first_with_percent() {
        let conn = setup();
        let (wrench_id, driver_id) = fixture(&conn);

        // 查询「修水管」同时传向量：扳手走语义通道，螺丝刀走关键词通道。
        let results = search(&conn, "修水管", Some(&[1.0, 0.0, 0.0])).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, wrench_id);
        assert_eq!(results[0].matched_by, MatchedBy::Semantic);
        assert_eq!(results[0].similarity_percent, Some(71));
        assert_eq!(results[0].categories, vec!["五金".to_string()]);
        assert_eq!(results[1].id, driver_id);
        assert_eq!(results[1].matched_by, MatchedBy::Keyword);
        assert!(results[1].similarity_percent.is_none());
    }

    #[test]
    fn dedup_prefers_semantic() {
        let conn = setup();
        let (wrench_id, _) = fixture(&conn);

        // 「扳手」既被关键词命中又被语义命中：只出现一次，且为语义。
        let results = search(&conn, "扳手", Some(&[1.0, 0.0, 0.0])).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, wrench_id);
        assert_eq!(results[0].matched_by, MatchedBy::Semantic);
        assert_eq!(results[0].similarity_percent, Some(71));
    }

    #[test]
    fn semantic_sorted_by_similarity_desc() {
        let conn = setup();
        fixture(&conn);
        let unit_id: i64 = conn
            .query_row("SELECT id FROM storage_units", [], |r| r.get(0))
            .unwrap();
        let box_ = boxes::create_box(&conn, unit_id, "杂物箱", "").unwrap();
        let full = items::create_item(&conn, box_.id, "管钳", "", 1, &[]).unwrap();
        set_embedding(&conn, full.id, &[1.0, 0.0, 0.0]); // 相似度 1.0

        let results = search(&conn, "", Some(&[1.0, 0.0, 0.0])).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, full.id);
        assert_eq!(results[0].similarity_percent, Some(100));
        assert_eq!(results[1].similarity_percent, Some(71));
    }

    #[test]
    fn below_threshold_excluded() {
        let conn = setup();
        let (wrench_id, _) = fixture(&conn);
        // 正交向量：相似度 0 < 0.3，被过滤。
        let results = search(&conn, "", Some(&[0.0, 0.0, 1.0])).unwrap();
        assert!(results.is_empty());
        assert_ne!(wrench_id, 0);
    }

    #[test]
    fn corrupted_or_mismatched_embeddings_skipped() {
        let conn = setup();
        fixture(&conn);
        let unit_id: i64 = conn
            .query_row("SELECT id FROM storage_units", [], |r| r.get(0))
            .unwrap();
        let box_ = boxes::create_box(&conn, unit_id, "备用箱", "").unwrap();
        let bad = items::create_item(&conn, box_.id, "坏数据", "", 1, &[]).unwrap();
        conn.execute(
            "UPDATE items SET embedding = ?2 WHERE id = ?1",
            rusqlite::params![bad.id, vec![1u8, 2, 3]], // 非法长度
        )
        .unwrap();
        let dim = items::create_item(&conn, box_.id, "维度不同", "", 1, &[]).unwrap();
        set_embedding(&conn, dim.id, &[1.0]); // 与查询向量维度不一致

        let results = search(&conn, "", Some(&[1.0, 0.0, 0.0])).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "扳手");
    }

    #[test]
    fn empty_query_without_embedding() {
        let conn = setup();
        fixture(&conn);
        assert!(search(&conn, "", None).unwrap().is_empty());
        assert!(search(&conn, "  ", None).unwrap().is_empty());
    }
}
