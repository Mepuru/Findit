//! FRB 镜像的数据模型（Dart 端可见）。
//!
//! 说明：AI 相关的部分模型（`AiConfig` / `AiProvider` / `ParsedIntent` /
//! `EntityRef` / `QuickAddResult` / `ModifyResult`）直接定义在 `core::ai`
//! 子模块中，FRB 会随 API 签名一并镜像。

/// 存储单元（如“客厅柜子”“储藏室货架”）。
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub id: i64,
    pub name: String,
    pub description: String,
    /// 该单元下的收纳箱数量。
    pub box_count: i64,
}

/// 收纳箱（挂在某个存储单元下，`slug` 用于二维码）。
/// 命名加 `Storage` 前缀以避免与 `std::boxed::Box` 冲突。
#[derive(Debug, Clone, PartialEq)]
pub struct StorageBox {
    pub id: i64,
    /// UUID v4，建箱时生成，用于二维码内容。
    pub slug: String,
    pub name: String,
    pub description: String,
    pub unit_id: i64,
    /// 该箱内物品数量。
    pub item_count: i64,
    /// ISO 8601 UTC 字符串。
    pub created_at: String,
    /// ISO 8601 UTC 字符串。
    pub updated_at: String,
}

/// 物品（放在某个收纳箱内）。不含 `embedding`。
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub quantity: i64,
    /// 照片相对路径；未上传照片时为 `None`。
    pub photo_path: Option<String>,
    pub box_id: i64,
    /// 物品所属分类名列表。
    pub categories: Vec<String>,
    /// ISO 8601 UTC 字符串。
    pub created_at: String,
    /// ISO 8601 UTC 字符串。
    pub updated_at: String,
}

/// 分类。
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub id: i64,
    pub name: String,
}

/// 搜索命中方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    /// 语义向量命中，附带相似度百分比。
    Semantic,
    /// 关键词命中。
    Keyword,
}

/// 一条搜索结果：物品完整信息 + 所在箱/单元名 + 命中方式。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub quantity: i64,
    /// 照片相对路径；未上传照片时为 `None`。
    pub photo_path: Option<String>,
    pub box_id: i64,
    /// 物品所属分类名列表。
    pub categories: Vec<String>,
    /// 所在收纳箱名（便于结果定位）。
    pub box_name: String,
    /// 所在存储单元名（便于结果定位）。
    pub unit_name: String,
    pub matched_by: MatchedBy,
    /// 语义命中时的相似度百分比（0-100）；关键词命中为 `None`。
    pub similarity_percent: Option<i32>,
}

/// AI 连接探活结果：对话与向量各自可用性 + 错误信息。
#[derive(Debug, Clone, PartialEq)]
pub struct AiTestResult {
    pub chat_ok: bool,
    pub chat_message: String,
    pub embed_ok: bool,
    pub embed_message: String,
}

/// AI 总体状态：配置概要 + 短缓存的探活结果。
#[derive(Debug, Clone, PartialEq)]
pub struct AiStatus {
    /// 服务地址非空即视为已配置。
    pub configured: bool,
    pub provider: crate::core::ai::config::AiProvider,
    pub base_url: String,
    pub chat_model: String,
    pub embed_model: String,
    /// 向量服务实际使用的 Provider（回退对话 Provider）。
    pub embed_provider: crate::core::ai::config::AiProvider,
    /// 向量服务实际使用的地址。
    pub embed_base_url: String,
    /// 库存向量实际使用的模型名（未回填过为 `None`）。
    pub embedded_model: Option<String>,
    /// 库存向量维度。
    pub embedded_dim: Option<i64>,
    /// 待回填物品数。
    pub pending_embeddings: i64,
    /// 缓存探活：对话可用性（未探活为 `None`）。
    pub last_chat_ok: Option<bool>,
    /// 缓存探活：向量可用性。
    pub last_embed_ok: Option<bool>,
    /// 缓存探活距今秒数（无缓存为 `None`）。
    pub cache_age_secs: Option<i64>,
}

/// 向量重建进度（流式推送）。
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedProgress {
    pub done: i32,
    pub total: i32,
}

/// 向量重建汇总。
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedProgressSummary {
    pub processed: i32,
    pub total: i32,
}

/// 备份导出汇总。
#[derive(Debug, Clone, PartialEq)]
pub struct BackupSummary {
    pub items_count: i64,
    pub boxes_count: i64,
    pub units_count: i64,
    /// 打进 zip 的照片文件数（含缩略图）。
    pub photos_count: i32,
    /// zip 内未压缩总字节数。
    pub total_bytes: i64,
    /// 实际写入的备份文件路径。
    pub path: String,
}

/// 备份导出进度（流式推送）。
///
/// `stage` ∈ `snapshot` / `photos` / `finalize` / `done`；
/// 最后一条事件（`stage == "done"`）携带完整 [`BackupSummary`]
/// （FRB 的流式函数返回值不会到达 Dart，故随流下发）。
#[derive(Debug, Clone, PartialEq)]
pub struct BackupProgress {
    pub stage: String,
    pub done: i32,
    pub total: i32,
    pub summary: Option<BackupSummary>,
}

/// 备份恢复汇总。
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreSummary {
    /// 恢复成功标记：Dart 侧据此提示并刷新全部页面数据。
    pub restored: bool,
    /// 恢复后库中的库存计数。
    pub items_count: i64,
    pub boxes_count: i64,
    pub units_count: i64,
    /// 恢复的照片文件数（含缩略图）。
    pub photos_count: i32,
}

/// 备份恢复进度（流式推送）。
///
/// `stage` ∈ `validate` / `extract` / `swap` / `done`；
/// 最后一条事件（`stage == "done"`）携带完整 [`RestoreSummary`]。
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreProgress {
    pub stage: String,
    pub done: i32,
    pub total: i32,
    pub summary: Option<RestoreSummary>,
}
