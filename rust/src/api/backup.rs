//! 备份/恢复 API 薄壳：转发 `core::backup`，进度经 [`StreamSink`] 下发。
//!
//! 说明：FRB 流式函数的返回值不会到达 Dart 端，因此汇总（`BackupSummary` /
//! `RestoreSummary`）附在最后一条进度事件（`stage == "done"`）中下发；
//! 函数返回值仅用于 Rust 侧的错误语义。

use std::path::Path;

use rusqlite::Connection;

use crate::api::model::{BackupProgress, BackupSummary, RestoreProgress, RestoreSummary};
use crate::core::backup::restore::DEFAULT_RESTORE_LIMITS;
use crate::core::db::with_conn;
use crate::core::error::{FinditError, FinditResult};
use crate::frb_generated::StreamSink;

/// 导出备份到 `target_path`（zip）。进度阶段：`snapshot` / `photos` /
/// `finalize`，最后一条 `done` 事件携带 [`BackupSummary`]。
///
/// 锁边界（P-L4）：快照（`wal_checkpoint` + `VACUUM INTO` + 三表计数 +
/// 快照库内剔除 API Key）在**独立连接**上执行，全程不持有全局数据库锁；
/// 照片打包与 manifest 写入在无锁环境进行（快照文件已是一致性副本）。
pub fn export_backup(
    target_path: String,
    sink: StreamSink<BackupProgress>,
) -> Result<BackupSummary, FinditError> {
    let target_path = target_path.trim().to_string();
    if target_path.is_empty() {
        return Err(FinditError::Validation("备份路径不能为空".to_string()));
    }
    let dir = crate::core::db::db_dir()?;

    let progress = |stage: &str, done: i32, total: i32| {
        let _ = sink.add(BackupProgress {
            stage: stage.to_string(),
            done,
            total,
            summary: None,
        });
    };

    // 第一段（无全局锁）：独立连接生成快照 + 计数。
    progress("snapshot", 0, 1);
    let db_path = dir.join(crate::core::db::DB_FILE_NAME);
    let snapshot = crate::core::backup::export::create_snapshot(&db_path)?;
    progress("snapshot", 1, 1);

    // 第二段（锁外）：照片打包与 manifest 写入（可能耗时很长）。
    let stats = crate::core::backup::export::package_backup(
        snapshot,
        &dir,
        Path::new(&target_path),
        Some(&progress),
    )?;

    let summary = BackupSummary {
        items_count: stats.items_count,
        boxes_count: stats.boxes_count,
        units_count: stats.units_count,
        photos_count: stats.photos_count,
        total_bytes: stats.total_bytes,
        path: target_path,
    };
    let _ = sink.add(BackupProgress {
        stage: "done".to_string(),
        done: 1,
        total: 1,
        summary: Some(summary.clone()),
    });
    Ok(summary)
}

/// 从 `zip_path` 恢复备份：关闭当前连接 → 校验/解压/原子替换 →
/// 重新初始化数据库（迁移幂等）。进度阶段：`validate` / `extract` /
/// `swap`，最后一条 `done` 事件携带 [`RestoreSummary`]。
///
/// 恢复失败时正式数据目录不会被破坏，且会尽力重新打开原库。
pub fn restore_backup(
    zip_path: String,
    sink: StreamSink<RestoreProgress>,
) -> Result<RestoreSummary, FinditError> {
    let zip_path = zip_path.trim().to_string();
    if zip_path.is_empty() {
        return Err(FinditError::Validation("备份路径不能为空".to_string()));
    }
    let dir = crate::core::db::db_dir()?;

    // 先关连接，避免恢复期间持有旧库文件（也便于目录改名）。
    crate::core::db::close_db();
    let result = crate::core::backup::restore::restore_backup(
        Path::new(&zip_path),
        &dir,
        &DEFAULT_RESTORE_LIMITS,
        Some(&|stage, done, total| {
            let _ = sink.add(RestoreProgress {
                stage: stage.to_string(),
                done,
                total,
                summary: None,
            });
        }),
    );

    // 无论成败都重新初始化（失败时正式目录未变，重开原库）。
    let dir_str = dir.to_string_lossy().to_string();
    if let Err(reopen_err) = crate::core::db::init_db(&dir_str) {
        return Err(result.err().unwrap_or(reopen_err));
    }

    let stats = result?;
    let (items_count, boxes_count, units_count) = with_conn(count_rows)?;
    let photos_count = count_photo_files(&dir)?;

    let summary = RestoreSummary {
        restored: true,
        items_count,
        boxes_count,
        units_count,
        photos_count: stats.has_photos.then_some(photos_count).unwrap_or(0),
    };
    let _ = sink.add(RestoreProgress {
        stage: "done".to_string(),
        done: 1,
        total: 1,
        summary: Some(summary.clone()),
    });
    Ok(summary)
}

fn count_rows(conn: &Connection) -> FinditResult<(i64, i64, i64)> {
    let items: i64 = conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
    let boxes: i64 = conn.query_row("SELECT COUNT(*) FROM storage_boxes", [], |r| r.get(0))?;
    let units: i64 = conn.query_row("SELECT COUNT(*) FROM storage_units", [], |r| r.get(0))?;
    Ok((items, boxes, units))
}

fn count_photo_files(dir: &Path) -> FinditResult<i32> {
    let photos = dir.join(crate::core::db::PHOTOS_DIR_NAME);
    let entries = match std::fs::read_dir(&photos) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut n = 0i32;
    for entry in entries.flatten() {
        if entry.path().is_file() {
            n += 1;
        }
    }
    Ok(n)
}
