//! 语义搜索基础工具：余弦相似度、相似度阈值、`embedding` BLOB 互转。
//!
//! `items.embedding` 列以**小端 f32 数组**的 BLOB 形式存储，
//! 避免 JSON 字符串的解析与内存开销。
//!
//! ## 内存索引缓存（P-H3）
//!
//! 语义搜索最重的成本是「每次查询全量读库 + 全量计算」。这里维护一个进程内
//! [`EmbeddingIndex`] 缓存：一次性把全部向量读入内存并**预计算范数**，
//! 后续查询只在内存里做点积打分（不再触碰数据库），并支持 top-K 截断。
//!
//! 缓存失效策略（`mark_embeddings_dirty`）：
//! - 向量写入/清空路径（`core::ai::embed` 的 `write_item_embeddings` /
//!   `clear_all_embeddings`）主动 bump 版本号；
//! - 数据库重开/恢复（`core::db` 的 `init_db` / `close_db`）调用
//!   [`invalidate_embedding_cache`] 整体清空；
//! - 物品删除未 bump 版本时，下游按 id 回查数据库会跳过不存在的物品
//!   （见 `hybrid::search_with_index`），不会返回脏数据。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use rusqlite::Connection;

use crate::core::error::FinditResult;

/// 语义候选过滤阈值：相似度低于该值的物品不进入语义结果。
pub const SIMILARITY_THRESHOLD: f32 = 0.3;

/// 语义结果 top-K 上限：即使命中很多也只返回相似度最高的前 K 条。
pub const SEMANTIC_TOP_K: usize = 100;

/// 进程内向量索引（内存缓存单元）。
#[derive(Debug, Clone)]
pub struct EmbeddingIndex {
    /// 物品 id（与 `data` / `norms` 按行对齐）。
    ids: Vec<i64>,
    /// 向量维度（所有行一致；加载时丢弃维度不一致的行）。
    dim: usize,
    /// 行主序扁平存储，`row i` 位于 `data[i*dim .. (i+1)*dim]`。
    data: Vec<f32>,
    /// 预计算 L2 范数（`row i` 的范数）。
    norms: Vec<f32>,
}

impl EmbeddingIndex {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// 向量内容版本号：写入路径 bump 后，缓存视为过期。
static EMBEDDING_VERSION: AtomicU64 = AtomicU64::new(0);

/// `(version, index)` 缓存。
static EMBEDDING_CACHE: LazyLock<Mutex<Option<(u64, Arc<EmbeddingIndex>)>>> =
    LazyLock::new(|| Mutex::new(None));

/// 标记向量内容已变化（写入/清空后调用），使内存缓存下次查询时重建。
pub fn mark_embeddings_dirty() {
    EMBEDDING_VERSION.fetch_add(1, Ordering::Release);
}

/// 整体清空缓存（数据库重开/恢复后调用，避免跨库脏数据）。
pub fn invalidate_embedding_cache() {
    if let Ok(mut guard) = EMBEDDING_CACHE.lock() {
        *guard = None;
    }
    mark_embeddings_dirty();
}

/// 取当前向量索引：缓存命中（版本一致）时零数据库读取；
/// 未命中/过期时在调用方提供的连接上重建并回填缓存。
///
/// 注意：重建需要读库，调用方应自行决定锁边界（本函数不持全局库锁，
/// 由调用方通过 `with_conn` 传入连接）。
///
/// 内存库（`path() == Some("")`，测试场景）跳过缓存：各测试使用独立的
/// 临时库，而缓存是进程内全局的，版本号无法区分不同库的内容，缓存会让
/// 并行测试互相读到对方的数据；直接重建最安全。生产库为文件库，正常缓存。
pub fn cached_embedding_index(conn: &Connection) -> FinditResult<Arc<EmbeddingIndex>> {
    if conn.path() == Some("") {
        return Ok(Arc::new(load_embedding_index(conn)?));
    }
    let version = EMBEDDING_VERSION.load(Ordering::Acquire);
    {
        let guard = EMBEDDING_CACHE
            .lock()
            .expect("向量索引缓存互斥锁中毒");
        if let Some((v, idx)) = guard.as_ref() {
            if *v == version {
                return Ok(Arc::clone(idx));
            }
        }
    }
    // 缓存未命中或过期：重建（期间版本可能又变化，下次调用会再次重建，无害）。
    let idx = Arc::new(load_embedding_index(conn)?);
    let mut guard = EMBEDDING_CACHE
        .lock()
        .expect("向量索引缓存互斥锁中毒");
    *guard = Some((version, Arc::clone(&idx)));
    Ok(idx)
}

/// 从数据库全量加载向量索引（含预计算范数）。
///
/// 维度以第一条向量为准；维度不一致或损坏的 BLOB 被跳过
/// （与历史 `cosine_similarity` 的「维度不一致返回 0」行为一致）。
pub fn load_embedding_index(conn: &Connection) -> FinditResult<EmbeddingIndex> {
    let mut stmt = conn.prepare("SELECT id, embedding FROM items WHERE embedding IS NOT NULL")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut ids: Vec<i64> = Vec::with_capacity(rows.len());
    let mut data: Vec<f32> = Vec::with_capacity(rows.len() * 64);
    let mut norms: Vec<f32> = Vec::with_capacity(rows.len());
    let mut dim: Option<usize> = None;

    for (id, blob) in rows {
        let Some(vec) = blob_to_embedding(&blob) else {
            continue; // BLOB 损坏：跳过。
        };
        match dim {
            None => {
                dim = Some(vec.len());
            }
            Some(d) if d != vec.len() => continue, // 维度不一致：跳过。
            _ => {}
        }
        let mut norm = 0.0f32;
        for x in &vec {
            norm += x * x;
        }
        ids.push(id);
        data.extend_from_slice(&vec);
        norms.push(norm.sqrt());
    }

    Ok(EmbeddingIndex {
        ids,
        dim: dim.unwrap_or(0),
        data,
        norms,
    })
}

/// 对查询向量打分：与索引内全部向量计算余弦相似度（预计算范数），
/// 过滤低于阈值的条目，按相似度降序（同分按 id 稳定排序）取前 `k` 条。
///
/// 纯内存计算，不触碰数据库；调用方应在锁外执行。
pub fn score_top_k(index: &EmbeddingIndex, query_vec: &[f32], k: usize) -> Vec<(i64, f32)> {
    if query_vec.is_empty() || k == 0 || index.is_empty() || index.dim != query_vec.len() {
        return Vec::new();
    }
    let mut norm_q = 0.0f32;
    for x in query_vec {
        norm_q += x * x;
    }
    if norm_q == 0.0 {
        return Vec::new();
    }
    let norm_q = norm_q.sqrt();

    let mut scored: Vec<(i64, f32)> = Vec::with_capacity(index.len().min(k * 2));
    for (i, id) in index.ids.iter().enumerate() {
        let row = &index.data[i * index.dim..(i + 1) * index.dim];
        let mut dot = 0.0f32;
        for (x, y) in query_vec.iter().zip(row.iter()) {
            dot += x * y;
        }
        let sim = if index.norms[i] == 0.0 {
            0.0
        } else {
            (dot / (norm_q * index.norms[i])).clamp(0.0, 1.0)
        };
        if sim >= SIMILARITY_THRESHOLD {
            scored.push((*id, sim));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    scored.truncate(k);
    scored
}

/// 余弦相似度。
///
/// 返回值截断到 `[0, 1]`（负相关按 0 处理）；
/// 维度不一致、空向量或零向量一律返回 0。
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())).clamp(0.0, 1.0)
}

/// 是否达到语义候选阈值。
pub fn passes_threshold(similarity: f32) -> bool {
    similarity >= SIMILARITY_THRESHOLD
}

/// 相似度 → 整数百分比（0-100，四舍五入）。
pub fn similarity_percent(similarity: f32) -> i32 {
    (similarity.clamp(0.0, 1.0) * 100.0).round() as i32
}

/// 向量 → 小端字节序 BLOB。
pub fn embedding_to_blob(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// BLOB → 向量；长度非法（空或非 4 的倍数）时返回 `None`。
pub fn blob_to_embedding(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_is_one() {
        // f32 浮点累计误差内应趋近 1。
        let sim = cosine_similarity(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
        assert!((sim - 1.0).abs() < 1e-6);
        let sim2 = cosine_similarity(&[0.5, -0.25], &[0.5, -0.25]);
        assert!((sim2 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn cosine_opposite_clamps_to_zero() {
        // 反向向量原始值为 -1，按实现截断为 0。
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]), 0.0);
    }

    #[test]
    fn cosine_invalid_inputs_is_zero() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn threshold_boundary() {
        assert!(passes_threshold(SIMILARITY_THRESHOLD));
        assert!(passes_threshold(SIMILARITY_THRESHOLD + f32::EPSILON));
        assert!(!passes_threshold(SIMILARITY_THRESHOLD - 0.001));
        assert!(!passes_threshold(0.0));
    }

    #[test]
    fn percent_rounding() {
        assert_eq!(similarity_percent(1.0), 100);
        assert_eq!(similarity_percent(0.874), 87);
        assert_eq!(similarity_percent(0.875), 88);
        assert_eq!(similarity_percent(0.3), 30);
        assert_eq!(similarity_percent(-0.5), 0);
        assert_eq!(similarity_percent(2.0), 100);
    }

    #[test]
    fn blob_roundtrip() {
        let vec: Vec<f32> = vec![0.0, 1.5, -2.25, 1e-3, 1e30];
        let blob = embedding_to_blob(&vec);
        assert_eq!(blob.len(), vec.len() * 4);
        assert_eq!(blob_to_embedding(&blob), Some(vec));
    }

    #[test]
    fn blob_rejects_invalid_length() {
        assert_eq!(blob_to_embedding(&[]), None);
        assert_eq!(blob_to_embedding(&[1, 2, 3]), None);
        assert_eq!(blob_to_embedding(&[0; 5]), None);
    }

    // ------------------------------------------------------------------
    // P-H3：内存索引缓存 + 预计算范数 + top-K
    // ------------------------------------------------------------------

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, embedding BLOB);",
        )
        .unwrap();
        conn
    }

    fn insert_vec(conn: &rusqlite::Connection, id: i64, vec: &[f32]) {
        conn.execute(
            "INSERT INTO items (id, embedding) VALUES (?1, ?2)",
            rusqlite::params![id, embedding_to_blob(vec)],
        )
        .unwrap();
    }

    #[test]
    fn index_loads_with_precomputed_norms() {
        let conn = mem_conn();
        insert_vec(&conn, 1, &[3.0, 4.0]);
        insert_vec(&conn, 2, &[1.0, 0.0]);
        let idx = load_embedding_index(&conn).unwrap();
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.dim, 2);
        // 范数 5 与 1
        assert!((idx.norms[0] - 5.0).abs() < 1e-6);
        assert!((idx.norms[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn index_skips_corrupt_and_dim_mismatch_rows() {
        let conn = mem_conn();
        insert_vec(&conn, 1, &[1.0, 0.0, 0.0]); // 维度 3（基准）
        conn.execute(
            "INSERT INTO items (id, embedding) VALUES (2, ?1)",
            rusqlite::params![vec![1u8, 2, 3]],
        )
        .unwrap(); // 损坏 BLOB
        insert_vec(&conn, 3, &[0.0, 1.0]); // 维度 2 ≠ 3
        insert_vec(&conn, 4, &[0.0, 0.0, 1.0]); // 维度 3
        let idx = load_embedding_index(&conn).unwrap();
        assert_eq!(idx.ids, vec![1, 4]);
        assert_eq!(idx.dim, 3);
    }

    #[test]
    fn score_top_k_filters_sorts_and_truncates() {
        let conn = mem_conn();
        insert_vec(&conn, 1, &[1.0, 0.0, 0.0]); // 与查询完全一致 → 1.0
        insert_vec(&conn, 2, &[0.0, 1.0, 0.0]); // 正交 → 0
        insert_vec(&conn, 3, &[0.8, 0.0, 0.6]); // ~0.8
        insert_vec(&conn, 4, &[0.5, 0.5, 0.0]); // ~0.707
        let idx = load_embedding_index(&conn).unwrap();
        let scored = score_top_k(&idx, &[1.0, 0.0, 0.0], 2);
        assert_eq!(scored.len(), 2);
        assert_eq!(scored[0].0, 1); // 相似度最高在前
        assert_eq!(scored[1].0, 3);
        assert!((scored[0].1 - 1.0).abs() < 1e-5);
        // 正交被阈值过滤
        assert!(!scored.iter().any(|(id, _)| *id == 2));
    }

    #[test]
    fn score_top_k_dim_mismatch_returns_empty() {
        let conn = mem_conn();
        insert_vec(&conn, 1, &[1.0, 0.0, 0.0]);
        let idx = load_embedding_index(&conn).unwrap();
        assert!(score_top_k(&idx, &[1.0, 0.0], 10).is_empty());
    }

    #[test]
    fn cache_reloads_after_mark_dirty() {
        let conn = mem_conn();
        invalidate_embedding_cache(); // 清掉其它测试可能留下的缓存
        insert_vec(&conn, 1, &[1.0, 0.0]);
        let idx1 = cached_embedding_index(&conn).unwrap();
        assert_eq!(idx1.len(), 1);

        // 同一版本再次读取：内容一致（内存库跳过缓存、每次直接重建）。
        let idx2 = cached_embedding_index(&conn).unwrap();
        assert_eq!(idx2.len(), 1);

        // 写入向量后 bump 版本：重建
        insert_vec(&conn, 2, &[0.0, 1.0]);
        mark_embeddings_dirty();
        let idx3 = cached_embedding_index(&conn).unwrap();
        assert_eq!(idx3.len(), 2);
    }

    #[test]
    fn file_backed_db_uses_cache_and_rebuilds_on_dirty() {
        // 生产缓存路径（文件库）回归：同一版本不重建（缓存命中路径），
        // 写入并 bump 版本后重建。内存库跳过缓存，故用临时文件库验证。
        // 注意：版本号是进程内全局的，并行测试的 invalidate 可能 bump 版本
        // 导致多一次重建，因此这里只断言内容正确，不断言 Arc 恒等。
        let dir = std::env::temp_dir().join(format!(
            "findit-semantic-cache-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cache.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY, embedding BLOB);")
            .unwrap();

        invalidate_embedding_cache();
        insert_vec(&conn, 1, &[1.0, 0.0]);

        let idx1 = cached_embedding_index(&conn).unwrap();
        assert_eq!(idx1.len(), 1);
        // 同一版本再次读取：要么命中缓存、要么被并行 bump 触发一次重建，
        // 内容都必须来自本测试的库。
        let idx2 = cached_embedding_index(&conn).unwrap();
        assert_eq!(idx2.len(), 1);

        insert_vec(&conn, 2, &[0.0, 1.0]);
        mark_embeddings_dirty();
        let idx3 = cached_embedding_index(&conn).unwrap();
        assert_eq!(idx3.len(), 2, "版本变化后应重建索引");

        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
