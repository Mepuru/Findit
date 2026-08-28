//! 恢复备份：校验链 → 解到临时目录 → 完整性校验 → 原子替换。
//!
//! 校验链（任一失败即拒绝，正式数据目录不动）：
//! 1. 扩展名必须为 `.zip`；
//! 2. 文件大小不超过 `max_zip_bytes`（默认 1GB）；
//! 3. 逐条目路径规范化：拒绝 `..`、绝对路径、盘符（防 zip-slip）；
//! 4. 累计解压大小不超过 `max_uncompressed_bytes`（默认 2GB），
//!    且压缩比不超过 `max_compression_ratio`（默认 100，防 zip-bomb）；
//! 5. 必须包含 `findit.db`；
//! 6. `manifest.json` 必须存在且 `format_version` 不高于当前支持版本；
//! 7. 解压后的 `findit.db` 必须能被 SQLite 打开且 `integrity_check` 通过，
//!    `user_version` 不高于当前 schema 版本，且 6 张必需表全部存在。
//!
//! 原子替换：当前数据目录先改名为 `{db_dir}.backup-{unix秒}` 保留一份副本，
//! 临时目录改名顶替；任一改名失败立即回滚。

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use uuid::Uuid;
use zip::ZipArchive;

use crate::core::backup::{DB_ENTRY_NAME, FORMAT_VERSION, MANIFEST_ENTRY_NAME, STREAM_BUF_SIZE};
use crate::core::db::migrations::CURRENT_VERSION;
use crate::core::error::{FinditError, FinditResult};

/// 恢复前必须存在的表（迁移 v1 建表清单）。
const REQUIRED_TABLES: [&str; 6] = [
    "storage_units",
    "storage_boxes",
    "items",
    "categories",
    "item_categories",
    "app_settings",
];

/// 恢复上限（可注入，便于测试用小值）。
#[derive(Debug, Clone, Copy)]
pub struct RestoreLimits {
    /// zip 文件本身的最大字节数。
    pub max_zip_bytes: u64,
    /// 累计解压（未压缩）最大字节数。
    pub max_uncompressed_bytes: u64,
    /// 最大压缩比（未压缩/压缩），超过视为炸弹。
    pub max_compression_ratio: u64,
    /// zip 条目（文件/目录）数上限（S-L2）：防止海量微小条目 DoS。
    /// 每条目对应一个待解压文件，条目数上限即文件数预算。
    pub max_entry_count: u64,
}

/// 生产默认上限：1GB zip / 2GB 解压 / 100 倍压缩比 / 10 万条目（S-L2）。
pub const DEFAULT_RESTORE_LIMITS: RestoreLimits = RestoreLimits {
    max_zip_bytes: 1 << 30,
    max_uncompressed_bytes: 2 << 30,
    max_compression_ratio: 100,
    max_entry_count: 100_000,
};

/// 恢复统计。
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreStats {
    /// 实际解压的文件数。
    pub files_extracted: i32,
    /// 实际解压字节数。
    pub total_bytes: i64,
    /// 备份是否包含照片目录。
    pub has_photos: bool,
}

/// 从 `zip_path` 恢复备份，原子替换 `db_dir`。
///
/// `on_progress(stage, done, total)`：stage ∈ `validate` / `extract` / `swap`。
/// 任何失败都会清理临时目录且不动正式数据目录。
pub fn restore_backup(
    zip_path: &Path,
    db_dir: &Path,
    limits: &RestoreLimits,
    on_progress: Option<&dyn Fn(&str, i32, i32)>,
) -> FinditResult<RestoreStats> {
    if let Some(f) = on_progress {
        f("validate", 0, 1);
    }

    // 1. 扩展名。
    let ext = zip_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "zip" {
        return Err(FinditError::Validation(
            "仅支持 .zip 格式的备份文件".to_string(),
        ));
    }

    // 2. 大小上限。
    let meta = std::fs::metadata(zip_path).map_err(|e| {
        FinditError::Validation(format!("无法读取备份文件：{e}"))
    })?;
    if !meta.is_file() {
        return Err(FinditError::Validation("备份路径不是文件".to_string()));
    }
    if meta.len() > limits.max_zip_bytes {
        return Err(FinditError::Validation(format!(
            "备份文件过大（{} 字节，上限 {} 字节）",
            meta.len(),
            limits.max_zip_bytes
        )));
    }

    // 3. 打开 zip。
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(BufReader::new(file))
        .map_err(|e| FinditError::Validation(format!("无法打开备份 zip：{e}")))?;

    // 4. 第一遍：安全与规模校验。
    // S-L2：先核对 zip 条目数（文件数预算），再逐条目做路径/字节校验。
    if (archive.len() as u64) > limits.max_entry_count {
        return Err(FinditError::Validation(format!(
            "备份条目数过多（{} 条，上限 {} 条）",
            archive.len(),
            limits.max_entry_count
        )));
    }
    let mut has_db = false;
    let mut has_photos = false;
    let mut total_uncompressed: u64 = 0;
    let mut total_compressed: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| FinditError::Validation(format!("备份 zip 条目损坏：{e}")))?;
        let name = sanitize_entry_name(entry.name())?;
        let Some(name) = name else { continue }; // 目录条目
        if name == DB_ENTRY_NAME {
            has_db = true;
        }
        if name.starts_with("photos/") {
            has_photos = true;
        }
        total_uncompressed = total_uncompressed
            .checked_add(entry.size())
            .ok_or_else(|| FinditError::Validation("备份解压大小溢出".to_string()))?;
        total_compressed = total_compressed.saturating_add(entry.compressed_size());
        if total_uncompressed > limits.max_uncompressed_bytes {
            return Err(FinditError::Validation(format!(
                "备份解压后超过大小上限（{} 字节）",
                limits.max_uncompressed_bytes
            )));
        }
    }
    if !has_db {
        return Err(FinditError::Validation(
            "备份缺少数据库文件 findit.db，不是有效的 Findit 备份".to_string(),
        ));
    }
    let bomb_like = (total_compressed == 0 && total_uncompressed > 0)
        || total_uncompressed > limits.max_compression_ratio * total_compressed;
    if bomb_like {
        return Err(FinditError::Validation(
            "备份压缩比异常（疑似恶意文件），已拒绝".to_string(),
        ));
    }

    // 5. 解压到临时目录（与正式目录同父目录，保证后续改名不跨卷）。
    let parent = db_dir.parent().filter(|p| !p.as_os_str().is_empty());
    let temp_dir = match parent {
        Some(p) => p.join(format!(".findit-restore-{}", Uuid::new_v4())),
        None => return Err(FinditError::Validation("数据目录路径无效".to_string())),
    };
    std::fs::create_dir_all(&temp_dir)?;

    let extract = extract_all(&mut archive, &temp_dir, limits, on_progress);
    let stats = match extract {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(e);
        }
    };

    // 6. 校验解压出的备份内容（manifest + 数据库 schema）。
    if let Err(e) = validate_extracted_backup(&temp_dir) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(e);
    }

    // 7. 原子替换。
    if let Some(f) = on_progress {
        f("swap", 0, 1);
    }
    let leftover = swap_dirs(&temp_dir, db_dir);
    let leftover = match leftover {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(e);
        }
    };

    // 防御（S-H3）：恢复的库若残留密钥类设置（如旧版导出的备份未剔除），
    // 立即置空，保证恢复后的库不含任何密钥。导出端已保证剔除，此处失败不阻断。
    if let Ok(restored_conn) = Connection::open(db_dir.join(DB_ENTRY_NAME)) {
        if let Err(e) = crate::core::backup::export::scrub_secrets(&restored_conn) {
            eprintln!("[findit] 恢复后清理密钥设置失败：{e}");
        }
    }

    // S-L1：旧数据副本移入数据目录内的隐藏目录 `.old-data-{ts}`。
    // 数据目录位于平台云备份排除范围（Android allowBackup=false 排除整个应用数据目录；
    // iOS AppDelegate 对 Documents 目录设置 NSURLIsExcludedFromBackupKey，而数据目录即
    // Documents），因此副本不再参与云备份；同时仍保留一份用于恢复后的回滚。
    // 移动失败不阻断恢复（副本保留在原位，下次恢复时仍会被清理）。
    if let Err(e) = relocate_leftover_backup(db_dir, &leftover) {
        eprintln!("[findit] 旧数据副本移入数据目录失败（副本保留在原位）：{e}");
    }

    if let Some(f) = on_progress {
        f("swap", 1, 1);
    }
    Ok(RestoreStats {
        files_extracted: stats.0,
        total_bytes: stats.1,
        has_photos,
    })
}

/// 条目名规范化：`\\` → `/`；拒绝绝对路径、盘符、`..` 组件。
/// 返回目录条目（空名）时返回 `None`。
pub(crate) fn sanitize_entry_name(raw: &str) -> FinditResult<Option<String>> {
    let name = raw.replace('\\', "/");
    // 以 `/` 结尾的是目录条目，不参与解压。
    if name.ends_with('/') {
        return Ok(None);
    }
    let name = name.trim_matches('/');
    if name.is_empty() {
        return Ok(None);
    }
    let mut clean = Vec::new();
    for comp in name.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            return Err(FinditError::Validation(format!(
                "备份包含非法路径条目：{raw}"
            )));
        }
        if comp.contains(':') {
            return Err(FinditError::Validation(format!(
                "备份包含非法路径条目（盘符）：{raw}"
            )));
        }
        clean.push(comp);
    }
    Ok(Some(clean.join("/")))
}

/// 第二遍：逐条目解压到 `temp_dir`，64KB 流式 + 实时字节预算。
/// 返回 (文件数, 字节数)。
fn extract_all<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    temp_dir: &Path,
    limits: &RestoreLimits,
    on_progress: Option<&dyn Fn(&str, i32, i32)>,
) -> FinditResult<(i32, i64)> {
    let total = archive.len() as i32;
    let mut files = 0i32;
    let mut written: u64 = 0;
    let mut buf = [0u8; STREAM_BUF_SIZE];

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| FinditError::Validation(format!("备份 zip 条目损坏：{e}")))?;
        let name = sanitize_entry_name(entry.name())?;
        let Some(name) = name else { continue };

        let dest = temp_dir.join(&name);
        // 双保险：目标必须仍在临时目录内。
        if !dest.starts_with(temp_dir) {
            return Err(FinditError::Validation(format!(
                "备份包含非法路径条目：{name}"
            )));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut out = File::create(&dest)?;
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            written = written.checked_add(n as u64).ok_or_else(|| {
                FinditError::Validation("备份解压大小溢出".to_string())
            })?;
            if written > limits.max_uncompressed_bytes {
                return Err(FinditError::Validation(format!(
                    "备份解压后超过大小上限（{} 字节）",
                    limits.max_uncompressed_bytes
                )));
            }
            out.write_all(&buf[..n])?;
        }
        files += 1;
        if let Some(f) = on_progress {
            f("extract", files, total);
        }
    }
    Ok((files, written as i64))
}

/// 校验解压出的备份内容：
/// 1. `manifest.json` 存在、可解析，`format_version` 缺失或过高时拒绝；
/// 2. `findit.db` 能被 SQLite 打开且 `integrity_check` 通过；
/// 3. `user_version` 不高于当前 schema 版本（拒绝来自更新版本应用的备份）；
/// 4. 6 张必需表全部存在。
fn validate_extracted_backup(temp_dir: &Path) -> FinditResult<()> {
    // 1. manifest.json。
    let manifest_raw =
        std::fs::read_to_string(temp_dir.join(MANIFEST_ENTRY_NAME)).map_err(|_| {
            FinditError::Validation("备份缺少 manifest.json，不是有效的 Findit 备份".to_string())
        })?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_raw)
        .map_err(|e| FinditError::Validation(format!("备份 manifest.json 无法解析：{e}")))?;
    match manifest.get("format_version").and_then(|v| v.as_i64()) {
        Some(v) if v <= FORMAT_VERSION => {}
        Some(v) => {
            return Err(FinditError::Validation(format!(
                "备份格式版本过新（format_version={v}，当前支持 {FORMAT_VERSION}），请先升级应用"
            )))
        }
        None => {
            return Err(FinditError::Validation(
                "备份 manifest.json 缺少 format_version".to_string(),
            ))
        }
    }

    // 2. 数据库可打开且完整性校验通过。
    let conn = Connection::open(temp_dir.join(DB_ENTRY_NAME))
        .map_err(|_| FinditError::Validation("备份数据库损坏：无法用 SQLite 打开".to_string()))?;
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| FinditError::Validation("备份数据库损坏：完整性校验失败".to_string()))?;
    if ok.trim().to_ascii_lowercase() != "ok" {
        return Err(FinditError::Validation(format!(
            "备份数据库损坏：{ok}"
        )));
    }

    // 3. user_version 不得高于当前支持的 schema 版本。
    let user_version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| {
            FinditError::Validation("备份数据库损坏：无法读取 user_version".to_string())
        })?;
    if user_version > CURRENT_VERSION {
        return Err(FinditError::Validation(format!(
            "备份数据库版本过新（user_version={user_version}，当前支持 {CURRENT_VERSION}），请先升级应用"
        )));
    }

    // 4. 必需表全部存在。
    for table in REQUIRED_TABLES {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |row| row.get(0),
        )?;
        if n == 0 {
            return Err(FinditError::Validation(format!(
                "备份数据库缺少必需表 {table}，不是有效的 Findit 备份"
            )));
        }
    }
    Ok(())
}

/// 原子替换：`db_dir` → `{db_dir}.backup-{unix秒}`，`temp_dir` → `db_dir`。
/// 第二步失败时立即把旧目录改回去（回滚）。
///
/// 返回旧数据副本目录路径（成功替换后由调用方决定如何处理，见 [`relocate_leftover_backup`]）。
fn swap_dirs(temp_dir: &Path, db_dir: &Path) -> FinditResult<PathBuf> {
    let parent = db_dir.parent().filter(|p| !p.as_os_str().is_empty());
    let dir_name = db_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| FinditError::Validation("数据目录路径无效".to_string()))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_dir = match parent {
        Some(p) => {
            // 同秒内多次恢复时避免命名冲突。
            let mut candidate = p.join(format!("{dir_name}.backup-{ts}"));
            let mut i = 1u32;
            while candidate.exists() {
                candidate = p.join(format!("{dir_name}.backup-{ts}-{i}"));
                i += 1;
            }
            candidate
        }
        None => return Err(FinditError::Validation("数据目录路径无效".to_string())),
    };

    std::fs::rename(db_dir, &backup_dir).map_err(|e| {
        FinditError::Io(format!("备份旧数据目录失败（恢复中止）：{e}"))
    })?;
    if let Err(e) = std::fs::rename(temp_dir, db_dir) {
        // 回滚：把旧目录改回原位。
        let _ = std::fs::rename(&backup_dir, db_dir);
        return Err(FinditError::Io(format!("替换数据目录失败：{e}")));
    }

    // 只保留最近一份旧数据副本，清理更早的 .backup-*（尽力而为）。
    cleanup_old_backups(db_dir, &backup_dir);
    Ok(backup_dir)
}

/// S-L1：把旧数据副本移入数据目录内的隐藏目录（`.old-data-{unix秒}`），
/// 使其处于平台云备份排除范围之内：
/// - Android：`allowBackup=false` 排除整个应用数据目录；
/// - iOS：AppDelegate 对 Documents（即数据目录）设置 `NSURLIsExcludedFromBackupKey`。
/// 保留最近一份用于恢复后的回滚；更早的 `.old-data-*` 一并清理（尽力而为）。
fn relocate_leftover_backup(db_dir: &Path, leftover: &Path) -> FinditResult<()> {
    // 副本内可能嵌套着更早的隐藏副本（前一次恢复移入的目录随整库被改名带走），
    // 一并清除，避免多层嵌套堆积。
    cleanup_old_hidden_backups(leftover);
    // 数据目录内已有的旧 `.old-data-*` 隐藏副本先清理（通常只剩这一份，防御残留）。
    cleanup_old_hidden_backups(db_dir);

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut dest = db_dir.join(format!(".old-data-{ts}"));
    let mut i = 1u32;
    while dest.exists() {
        dest = db_dir.join(format!(".old-data-{ts}-{i}"));
        i += 1;
    }
    std::fs::rename(leftover, &dest)
        .map_err(|e| FinditError::Io(format!("移动旧数据副本失败：{e}")))
}

/// 清理数据目录内旧的 `.old-data-*` 隐藏副本（保留最近一份是调用方职责，此处全清）。
fn cleanup_old_hidden_backups(db_dir: &Path) {
    let entries = match std::fs::read_dir(db_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".old-data-") {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// 清理旧的 `{dir_name}.backup-*` 目录，仅保留 `keep`。
fn cleanup_old_backups(db_dir: &Path, keep: &Path) {
    let Some(parent) = db_dir.parent() else { return };
    let dir_name = match db_dir.file_name().and_then(|s| s.to_str()) {
        Some(n) => n.to_string(),
        None => return,
    };
    let prefix = format!("{dir_name}.backup-");
    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep || !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}
