//! FRB 镜像的数据模型（Dart 端可见）。

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
