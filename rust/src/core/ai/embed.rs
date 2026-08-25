//! Embedding 生成与回填的纯逻辑：
//! 物品文本拼接、待处理扫描、批量写入、维度变更清空重建。
//!
//! 网络调用由 [`AiTransport`] 抽象注入（测试用 mock）；
//! 编排以「单轮回填」[`backfill_round`] 为单位，天然可恢复——
//! 待办以数据库 `embedding IS NULL` 为准，中断后重跑即可。

use rusqlite::{params, Connection};

use crate::core::ai::client::AiTransport;
use crate::core::ai::config::{AiConfig, KEY_EMBEDDED_DIM, KEY_EMBEDDED_MODEL};
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo::load_item_categories;
use crate::core::repo::settings::{get_setting, set_setting};
use crate::core::search::semantic::{blob_to_embedding, embedding_to_blob};

/// 默认批量：每次 `/api/embed` 调用的物品条数。
pub const DEFAULT_BATCH_SIZE: usize = 24;

/// 物品 → 向量文本：名称 + 备注 + 分类名（跳过空段，空格拼接）。
pub fn item_text(name: &str, description: &str, categories: &[String]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let name = name.trim();
    let description = description.trim();
    if !name.is_empty() {
        parts.push(name);
    }
    if !description.is_empty() {
        parts.push(description);
    }
    for cat in categories {
        let cat = cat.trim();
        if !cat.is_empty() {
            parts.push(cat);
        }
    }
    parts.join(" ")
}

/// 扫描待回填物品（`embedding IS NULL`），返回 (item_id, 向量文本)，
/// 最多 `limit` 条。
pub fn pending_item_texts(conn: &Connection, limit: usize) -> FinditResult<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description FROM items WHERE embedding IS NULL ORDER BY id LIMIT ?1",
    )?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map(params![limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let cats = load_item_categories(conn, &ids)?;

    Ok(rows
        .into_iter()
        .map(|(id, name, description)| {
            let categories = cats.get(&id).cloned().unwrap_or_default();
            (id, item_text(&name, &description, &categories))
        })
        .collect())
}

/// 批量写入物品向量（BLOB）。返回写入条数。
pub fn write_item_embeddings(conn: &Connection, pairs: &[(i64, Vec<f32>)]) -> FinditResult<usize> {
    let mut count = 0usize;
    for (item_id, vec) in pairs {
        let blob = embedding_to_blob(vec);
        conn.execute(
            "UPDATE items SET embedding = ?2 WHERE id = ?1",
            params![item_id, blob],
        )?;
        count += 1;
    }
    Ok(count)
}

/// 清空全部物品向量（模型/维度变更时先清空再重建）。返回受影响行数。
pub fn clear_all_embeddings(conn: &Connection) -> FinditResult<u64> {
    let affected = conn.execute("UPDATE items SET embedding = NULL WHERE embedding IS NOT NULL", [])?;
    Ok(affected as u64)
}

/// 物品总数。
pub fn count_items(conn: &Connection) -> FinditResult<i64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
    Ok(n)
}

/// 待回填物品数。
pub fn count_pending_embeddings(conn: &Connection) -> FinditResult<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM items WHERE embedding IS NULL",
        [],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// 库内既有向量的维度（无任何向量时为 `None`）。
pub fn stored_embedding_dim(conn: &Connection) -> FinditResult<Option<usize>> {
    let result = conn.query_row(
        "SELECT embedding FROM items WHERE embedding IS NOT NULL LIMIT 1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    );
    match result {
        Ok(blob) => Ok(blob_to_embedding(&blob).map(|v| v.len())),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 维度一致性检查：新向量维度与既有向量（或记录的维度）不一致时，
/// 清空全部旧向量；随后记录当前模型名与维度。
///
/// 返回是否发生了清空。
pub fn ensure_dim_consistent(
    conn: &Connection,
    new_dim: usize,
    model_name: &str,
) -> FinditResult<bool> {
    let existing_dim = stored_embedding_dim(conn)?;
    let recorded_dim: Option<usize> = get_setting(conn, KEY_EMBEDDED_DIM)?
        .and_then(|s| s.trim().parse().ok());

    let mismatch = match (existing_dim, recorded_dim) {
        (Some(e), _) if e != new_dim => true,
        (None, Some(r)) if r != new_dim => true,
        _ => false,
    };

    if mismatch {
        clear_all_embeddings(conn)?;
    }
    set_setting(conn, KEY_EMBEDDED_DIM, &new_dim.to_string())?;
    set_setting(conn, KEY_EMBEDDED_MODEL, model_name)?;
    Ok(mismatch)
}

/// 单轮回填：取一批待处理物品 → 调向量服务 → 维度检查 → 写库。
///
/// 返回本轮写入条数；为 0 表示已无待处理。网络失败直接返回错误，
/// 已写入的批次保留（可恢复，重跑即可）。
pub fn backfill_round(
    conn: &Connection,
    transport: &dyn AiTransport,
    config: &AiConfig,
    batch_size: usize,
) -> FinditResult<i32> {
    let pending = pending_item_texts(conn, batch_size)?;
    if pending.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();
    let vecs = transport
        .embed(config, &texts)
        .map_err(crate::core::ai::client::ai_error_to_findit)?;
    if vecs.len() != texts.len() {
        return Err(FinditError::AiModelOutput(format!(
            "向量数量不符：请求 {} 条，返回 {} 条",
            texts.len(),
            vecs.len()
        )));
    }
    if let Some(first) = vecs.first() {
        if first.is_empty() {
            return Err(FinditError::AiModelOutput("返回了空向量".to_string()));
        }
        ensure_dim_consistent(conn, first.len(), &config.embed_model)?;
    }
    let pairs: Vec<(i64, Vec<f32>)> = pending
        .iter()
        .map(|(id, _)| *id)
        .zip(vecs)
        .collect();
    let written = write_item_embeddings(conn, &pairs)?;
    Ok(written as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ai::client::AiError;
    use crate::core::ai::config::AiProvider;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::{boxes, categories, items, units};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// 测试用 mock：按序返回预设向量响应，并记录每批大小。
    struct MockTransport {
        responses: RefCell<VecDeque<Result<Vec<Vec<f32>>, AiError>>>,
        batches: RefCell<Vec<usize>>,
        /// 默认每条输入返回的向量维度。
        dim: usize,
    }

    impl MockTransport {
        fn new(dim: usize) -> Self {
            MockTransport {
                responses: RefCell::new(VecDeque::new()),
                batches: RefCell::new(Vec::new()),
                dim,
            }
        }

        fn push(&self, result: Result<Vec<Vec<f32>>, AiError>) {
            self.responses.borrow_mut().push_back(result);
        }
    }

    impl AiTransport for MockTransport {
        fn chat(&self, _config: &AiConfig, _system: &str, _user: &str) -> Result<String, AiError> {
            unimplemented!("embed 测试不使用 chat")
        }

        fn embed(&self, _config: &AiConfig, inputs: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            self.batches.borrow_mut().push(inputs.len());
            if let Some(next) = self.responses.borrow_mut().pop_front() {
                return next;
            }
            Ok(inputs
                .iter()
                .enumerate()
                .map(|(i, _)| vec![i as f32 + 1.0; self.dim])
                .collect())
        }
    }

    fn config() -> AiConfig {
        AiConfig {
            provider: AiProvider::Ollama,
            base_url: "http://mock:11434".to_string(),
            api_key: String::new(),
            chat_model: "chat-model".to_string(),
            embed_model: "embed-model".to_string(),
        }
    }

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn fixture_items(conn: &Connection, n: usize) -> Vec<i64> {
        let unit = units::create_unit(conn, "柜子", "").unwrap();
        let box_ = boxes::create_box(conn, unit.id, "箱子", "").unwrap();
        let cat = categories::create_category(conn, "工具").unwrap();
        (0..n)
            .map(|i| {
                let cat_ids: Vec<i64> = if i % 3 == 2 { vec![cat.id] } else { vec![] };
                items::create_item(
                    conn,
                    box_.id,
                    &format!("物品{i}"),
                    if i % 2 == 0 { "有备注" } else { "" },
                    1,
                    &cat_ids,
                )
                .unwrap()
                .id
            })
            .collect()
    }

    // ---------- 纯函数 ----------

    #[test]
    fn item_text_joins_nonempty_parts() {
        assert_eq!(
            item_text("扳手", "修水管的工具", &["五金".to_string(), "工具".to_string()]),
            "扳手 修水管的工具 五金 工具"
        );
        assert_eq!(item_text("扳手", "", &[]), "扳手");
        assert_eq!(item_text("扳手", "  ", &["  ".to_string()]), "扳手");
    }

    #[test]
    fn pending_scan_only_null_embeddings_with_batch_limit() {
        let conn = setup();
        let ids = fixture_items(&conn, 5);
        // 前两条已有向量
        write_item_embeddings(
            &conn,
            &[(ids[0], vec![1.0, 2.0]), (ids[1], vec![3.0, 4.0])],
        )
        .unwrap();

        let pending = pending_item_texts(&conn, 2).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].0, ids[2]);
        assert_eq!(pending[1].0, ids[3]);
        // 文本含名称/备注/分类
        assert_eq!(pending[0].1, "物品2 有备注 工具");
        assert_eq!(pending[1].1, "物品3");

        assert_eq!(count_pending_embeddings(&conn).unwrap(), 3);
        assert_eq!(count_items(&conn).unwrap(), 5);
    }

    #[test]
    fn write_and_clear_embeddings() {
        let conn = setup();
        let ids = fixture_items(&conn, 3);
        let n = write_item_embeddings(
            &conn,
            &[(ids[0], vec![0.5, 0.25]), (ids[1], vec![1.0, 0.0])],
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(stored_embedding_dim(&conn).unwrap(), Some(2));

        let cleared = clear_all_embeddings(&conn).unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(stored_embedding_dim(&conn).unwrap(), None);
        assert_eq!(count_pending_embeddings(&conn).unwrap(), 3);
    }

    #[test]
    fn dim_mismatch_clears_and_records() {
        let conn = setup();
        let ids = fixture_items(&conn, 3);
        write_item_embeddings(
            &conn,
            &[
                (ids[0], vec![1.0, 2.0]),
                (ids[1], vec![3.0, 4.0]),
            ],
        )
        .unwrap();
        set_setting(&conn, KEY_EMBEDDED_DIM, "2").unwrap();

        // 维度一致：不清空
        assert!(!ensure_dim_consistent(&conn, 2, "model-a").unwrap());
        assert_eq!(count_pending_embeddings(&conn).unwrap(), 1);

        // 维度变更：清空全部并记录新维度/模型
        assert!(ensure_dim_consistent(&conn, 8, "model-b").unwrap());
        assert_eq!(count_pending_embeddings(&conn).unwrap(), 3);
        assert_eq!(stored_embedding_dim(&conn).unwrap(), None);
        assert_eq!(
            get_setting(&conn, KEY_EMBEDDED_DIM).unwrap().as_deref(),
            Some("8")
        );
        assert_eq!(
            get_setting(&conn, KEY_EMBEDDED_MODEL).unwrap().as_deref(),
            Some("model-b")
        );
    }

    // ---------- 回填编排 ----------

    #[test]
    fn backfill_round_processes_batches_until_done() {
        let conn = setup();
        fixture_items(&conn, 5);
        let transport = MockTransport::new(4);

        // 批量 2 → 共 3 轮（2+2+1）
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 2);
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 2);
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 1);
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 0);

        assert_eq!(count_pending_embeddings(&conn).unwrap(), 0);
        assert_eq!(*transport.batches.borrow(), vec![2, 2, 1]);
        assert_eq!(stored_embedding_dim(&conn).unwrap(), Some(4));
        assert_eq!(
            get_setting(&conn, KEY_EMBEDDED_DIM).unwrap().as_deref(),
            Some("4")
        );
    }

    #[test]
    fn backfill_is_resumable_after_error() {
        let conn = setup();
        fixture_items(&conn, 4);
        let transport = MockTransport::new(2);
        transport.push(Ok(vec![vec![1.0, 1.0], vec![2.0, 2.0]]));
        transport.push(Err(AiError::Network("boom".into())));

        // 第一轮成功写入 2 条
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 2);
        // 第二轮网络失败：错误上抛，已写数据保留
        let err = backfill_round(&conn, &transport, &config(), 2).unwrap_err();
        assert!(matches!(err, FinditError::AiUnreachable(_)));
        assert_eq!(count_pending_embeddings(&conn).unwrap(), 2);

        // 恢复后重跑：继续处理剩余 2 条
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 2);
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 0);
        assert_eq!(count_pending_embeddings(&conn).unwrap(), 0);
    }

    #[test]
    fn backfill_clears_old_vectors_on_dim_change() {
        let conn = setup();
        let ids = fixture_items(&conn, 3);
        // 既有 2 维旧向量
        write_item_embeddings(&conn, &[(ids[0], vec![9.0, 9.0])]).unwrap();
        set_setting(&conn, KEY_EMBEDDED_DIM, "2").unwrap();

        // 新模型输出 3 维 → 旧向量被清空，全部重建
        let transport = MockTransport::new(3);
        let mut total = 0;
        loop {
            let n = backfill_round(&conn, &transport, &config(), 8).unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        assert_eq!(total, 3);
        assert_eq!(count_pending_embeddings(&conn).unwrap(), 0);
        assert_eq!(stored_embedding_dim(&conn).unwrap(), Some(3));
        // 旧向量的物品也被重建为新维度
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT embedding FROM items WHERE id = ?1",
                params![ids[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(blob_to_embedding(&blob).unwrap().len(), 3);
    }

    #[test]
    fn backfill_rejects_count_mismatch() {
        let conn = setup();
        fixture_items(&conn, 2);
        let transport = MockTransport::new(2);
        transport.push(Ok(vec![vec![1.0, 1.0]])); // 请求 2 条只返回 1 条
        let err = backfill_round(&conn, &transport, &config(), 2).unwrap_err();
        assert!(matches!(err, FinditError::AiModelOutput(_)));
        assert_eq!(count_pending_embeddings(&conn).unwrap(), 2);
    }

    #[test]
    fn backfill_no_pending_returns_zero_without_network() {
        let conn = setup();
        let transport = MockTransport::new(2);
        assert_eq!(backfill_round(&conn, &transport, &config(), 2).unwrap(), 0);
        assert!(transport.batches.borrow().is_empty());
    }
}
