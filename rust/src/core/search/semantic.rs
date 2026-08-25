//! 语义搜索基础工具：余弦相似度、相似度阈值、`embedding` BLOB 互转。
//!
//! `items.embedding` 列以**小端 f32 数组**的 BLOB 形式存储，
//! 避免 JSON 字符串的解析与内存开销。

/// 语义候选过滤阈值：相似度低于该值的物品不进入语义结果。
pub const SIMILARITY_THRESHOLD: f32 = 0.3;

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
}
