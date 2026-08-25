//! 导出备份：`findit.db` 一致性快照 + `photos/**` + `manifest.json` → zip。
//!
//! 防 WAL 丢数据：先 `PRAGMA wal_checkpoint(TRUNCATE)`，再 `VACUUM INTO`
//! 生成一致性快照文件打进 zip（不影响在线读写）。
//! 全程流式：固定 64KB 缓冲，内存占用恒定。

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use uuid::Uuid;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::CompressionMethod;

use crate::core::backup::{
    DB_ENTRY_NAME, FORMAT_VERSION, MANIFEST_ENTRY_NAME, PHOTOS_ENTRY_PREFIX, STREAM_BUF_SIZE,
};
use crate::core::db::PHOTOS_DIR_NAME;
use crate::core::error::{FinditError, FinditResult};
use crate::core::util::now_iso8601;

/// 导出统计（供上层组装汇总）。
#[derive(Debug, Clone, PartialEq)]
pub struct ExportStats {
    pub items_count: i64,
    pub boxes_count: i64,
    pub units_count: i64,
    /// 打进 zip 的照片文件数（含缩略图）。
    pub photos_count: i32,
    /// zip 内未压缩总字节数。
    pub total_bytes: i64,
}

/// 导出备份到 `target_path`（zip）。
///
/// `on_progress(stage, done, total)`：stage ∈ `snapshot` / `photos` / `finalize`。
/// 中途失败时尽力清理临时快照与残缺的目标文件。
pub fn export_backup(
    conn: &Connection,
    db_dir: &Path,
    target_path: &Path,
    on_progress: Option<&dyn Fn(&str, i32, i32)>,
) -> FinditResult<ExportStats> {
    if let Some(f) = on_progress {
        f("snapshot", 0, 1);
    }

    // 1. WAL 数据落盘，避免快照遗漏最近写入。
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;

    // 2. 统计计数（写入 manifest）。
    let (items_count, boxes_count, units_count) = count_rows(conn)?;

    // 3. VACUUM INTO 生成一致性快照（不阻塞在线读写）。
    let snapshot = std::env::temp_dir().join(format!("findit-snapshot-{}.db", Uuid::new_v4()));
    conn.execute(
        "VACUUM INTO ?1",
        [snapshot.to_string_lossy().as_ref()],
    )?;

    // 4. 收集照片文件（排序保证条目顺序稳定）。
    let photos = list_photo_files(&db_dir.join(PHOTOS_DIR_NAME))?;

    let result = write_zip(
        &snapshot,
        &photos,
        target_path,
        (items_count, boxes_count, units_count),
        |stage, done, total| {
            if let Some(f) = on_progress {
                f(stage, done, total);
            }
        },
    );

    // 无论成败都清理快照；失败时顺带删除残缺的目标文件。
    let _ = std::fs::remove_file(&snapshot);
    match result {
        Ok(stats) => Ok(ExportStats {
            items_count,
            boxes_count,
            units_count,
            photos_count: photos.len() as i32,
            total_bytes: stats,
        }),
        Err(e) => {
            let _ = std::fs::remove_file(target_path);
            Err(e)
        }
    }
}

fn count_rows(conn: &Connection) -> FinditResult<(i64, i64, i64)> {
    let items: i64 = conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
    let boxes: i64 = conn.query_row("SELECT COUNT(*) FROM storage_boxes", [], |r| r.get(0))?;
    let units: i64 = conn.query_row("SELECT COUNT(*) FROM storage_units", [], |r| r.get(0))?;
    Ok((items, boxes, units))
}

/// 列出目录下的普通文件（按文件名排序）；目录不存在返回空列表。
fn list_photo_files(dir: &Path) -> FinditResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() {
                    files.push(path);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(e) => return Err(e.into()),
    }
    files.sort();
    Ok(files)
}

/// 顺序写入 zip：`findit.db` → `photos/**` → `manifest.json`。
/// 返回未压缩总字节数。
fn write_zip(
    snapshot: &Path,
    photos: &[PathBuf],
    target_path: &Path,
    counts: (i64, i64, i64),
    mut on_progress: impl FnMut(&str, i32, i32),
) -> FinditResult<i64> {
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let target = File::create(target_path)?;
    let mut zip = ZipWriter::new(target);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut total: i64 = 0;

    // findit.db
    zip.start_file(DB_ENTRY_NAME, options)
        .map_err(zip_err)?;
    let mut src = File::open(snapshot)?;
    total += copy_stream(&mut src, &mut zip)? as i64;

    // photos/**
    let n = photos.len() as i32;
    for (i, path) in photos.iter().enumerate() {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| FinditError::Io(format!("非法的照片文件名：{}", path.display())))?;
        on_progress("photos", i as i32, n);
        zip.start_file(format!("{PHOTOS_ENTRY_PREFIX}{name}"), options)
            .map_err(zip_err)?;
        let mut src = File::open(path)?;
        total += copy_stream(&mut src, &mut zip)? as i64;
    }
    on_progress("photos", n, n);

    // manifest.json
    on_progress("finalize", 0, 1);
    let manifest = build_manifest(counts)?;
    zip.start_file(MANIFEST_ENTRY_NAME, options)
        .map_err(zip_err)?;
    zip.write_all(&manifest)?;
    total += manifest.len() as i64;

    zip.finish().map_err(zip_err)?;
    on_progress("finalize", 1, 1);
    Ok(total)
}

/// 生成 `manifest.json`：格式版本 + 导出时间 + 库存计数。
fn build_manifest((items, boxes, units): (i64, i64, i64)) -> FinditResult<Vec<u8>> {
    let manifest = serde_json::json!({
        "app": "findit",
        "format_version": FORMAT_VERSION,
        "exported_at": now_iso8601(),
        "items_count": items,
        "boxes_count": boxes,
        "units_count": units,
    });
    serde_json::to_vec(&manifest).map_err(|e| FinditError::Io(format!("manifest 序列化失败：{e}")))
}

/// 固定 64KB 缓冲的流式拷贝，返回拷贝字节数。
fn copy_stream<R: Read, W: Write>(src: &mut R, dst: &mut W) -> FinditResult<u64> {
    let mut buf = [0u8; STREAM_BUF_SIZE];
    let mut written = 0u64;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        written += n as u64;
    }
    Ok(written)
}

fn zip_err(e: zip::result::ZipError) -> FinditError {
    FinditError::Io(format!("zip 写入失败：{e}"))
}
