//! 备份与恢复（阶段 5）。
//!
//! - [`export`]：把当前数据目录（`findit.db` 一致性快照 + `photos/**`）
//!   流式打成单个 zip，附带 `manifest.json` 元信息；
//! - [`restore`]：校验 → 解到临时目录 → 校验数据库完整性 → 原子替换
//!   正式数据目录（旧目录保留为 `{db_dir}.backup-{时间戳}` 副本）。
//!
//! 两个入口都是纯函数（显式接收 `&Connection` / 路径），不依赖全局状态，
//! 可用临时目录独立做 `cargo test`。

pub mod export;
pub mod restore;

/// 流式读写的固定缓冲：64KB，保证内存占用恒定。
pub const STREAM_BUF_SIZE: usize = 64 * 1024;

/// zip 内数据库条目名（固定，恢复时据此定位）。
pub const DB_ENTRY_NAME: &str = "findit.db";
/// zip 内清单条目名。
pub const MANIFEST_ENTRY_NAME: &str = "manifest.json";
/// zip 内照片目录前缀。
pub const PHOTOS_ENTRY_PREFIX: &str = "photos/";

/// 备份格式版本（写入 `manifest.json`）。
pub const FORMAT_VERSION: i64 = 1;

#[cfg(test)]
mod tests;
