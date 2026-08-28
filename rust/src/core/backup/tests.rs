//! 备份/恢复集成测试：往返一致性、WAL、安全校验、回滚。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use uuid::Uuid;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

use super::export::export_backup;
use super::restore::{restore_backup, RestoreLimits, DEFAULT_RESTORE_LIMITS};
use crate::core::db::migrations::run_migrations;
use crate::core::repo::{boxes, categories, items, settings, units};

// ---------------------------------------------------------------------------
// 测试基建
// ---------------------------------------------------------------------------

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("findit-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 打开（创建）一个 WAL 模式的文件数据库并应用迁移。
fn open_test_db(db_dir: &Path) -> Connection {
    fs::create_dir_all(db_dir.join("photos")).unwrap();
    let conn = Connection::open(db_dir.join("findit.db")).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "synchronous", "NORMAL").unwrap();
    run_migrations(&conn).unwrap();
    conn
}

/// 构造确定性伪随机字节（模拟照片文件）。
fn fake_photo_bytes(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| seed.wrapping_mul(31).wrapping_add((i % 251) as u8))
        .collect()
}

struct Seeded {
    unit_id: i64,
    box_id: i64,
    box_slug: String,
    item_ids: Vec<i64>,
    photo_main: String,
    photo_bytes_main: Vec<u8>,
    photo_bytes_thumb: Vec<u8>,
}

/// 造数据：单元/箱/分类/物品（含照片与向量）/设置。
fn seed_data(conn: &Connection, db_dir: &Path) -> Seeded {
    let unit = units::create_unit(conn, "客厅柜子", "沙发旁边的柜子").unwrap();
    let b = boxes::create_box(conn, unit.id, "冬装箱", "放冬天的衣服").unwrap();
    let cat = categories::create_category(conn, "衣物").unwrap();
    categories::create_category(conn, "电子产品").unwrap();

    let item1 =
        items::create_item(conn, b.id, "羽绒服", "加厚款", 1, &[cat.id]).unwrap();
    let item2 = items::create_item(conn, b.id, "暖手宝", "", 3, &[]).unwrap();

    // 照片文件（主图 + 缩略图）。
    let photo_bytes_main = fake_photo_bytes(7, 4096);
    let photo_bytes_thumb = fake_photo_bytes(13, 1024);
    let photo_main = format!("{}.jpg", Uuid::new_v4());
    let photo_thumb = format!("{}_thumb.jpg", photo_main.trim_end_matches(".jpg"));
    let photos = db_dir.join("photos");
    fs::write(photos.join(&photo_main), &photo_bytes_main).unwrap();
    fs::write(photos.join(&photo_thumb), &photo_bytes_thumb).unwrap();
    items::set_item_photo_path(conn, item1.id, Some(&photo_main)).unwrap();

    // 语义向量（BLOB）也应随备份往返。
    conn.execute(
        "UPDATE items SET embedding = ?1 WHERE id = ?2",
        rusqlite::params![vec![1u8, 2, 3, 4], item2.id],
    )
    .unwrap();

    settings::set_setting(conn, "ollama_url", "http://192.168.1.10:11434").unwrap();
    settings::set_setting(conn, "chat_model", "qwen3:4b").unwrap();

    Seeded {
        unit_id: unit.id,
        box_id: b.id,
        box_slug: b.slug.clone(),
        item_ids: vec![item1.id, item2.id],
        photo_main,
        photo_bytes_main,
        photo_bytes_thumb,
    }
}

fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

/// 断言目录里不存在任何 `{name}.backup-*` / `.findit-restore-*` 残留。
fn assert_no_backup_residue(db_dir: &Path) {
    let parent = db_dir.parent().unwrap();
    let name = db_dir.file_name().unwrap().to_string_lossy().to_string();
    for entry in fs::read_dir(parent).unwrap().flatten() {
        let n = entry.file_name().to_string_lossy().to_string();
        assert!(
            !n.starts_with(&format!("{name}.backup-")),
            "不应存在旧数据副本：{n}"
        );
        assert!(
            !n.starts_with(".findit-restore-"),
            "不应残留临时解压目录：{n}"
        );
    }
}

// ---------------------------------------------------------------------------
// 导出内容 / manifest
// ---------------------------------------------------------------------------

#[test]
fn export_contains_expected_entries_and_manifest() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");
    {
        let conn = open_test_db(&db_dir);
        seed_data(&conn, &db_dir);
        let stats = export_backup(&conn, &db_dir, &zip_path, None).unwrap();
        assert_eq!(stats.items_count, 2);
        assert_eq!(stats.boxes_count, 1);
        assert_eq!(stats.units_count, 1);
        assert_eq!(stats.photos_count, 2); // 主图 + 缩略图
        assert!(stats.total_bytes > 0);
    }

    let file = File::open(&zip_path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.contains(&"findit.db".to_string()));
    assert!(names.contains(&"manifest.json".to_string()));
    assert!(names.iter().any(|n| n.starts_with("photos/")));

    // manifest 内容。
    let mut manifest = String::new();
    archive
        .by_name("manifest.json")
        .unwrap()
        .read_to_string(&mut manifest)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(v["app"], "findit");
    assert_eq!(v["format_version"], 1);
    assert_eq!(v["items_count"], 2);
    assert_eq!(v["boxes_count"], 1);
    assert_eq!(v["units_count"], 1);
    assert!(v["exported_at"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn export_reports_progress_stages() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");
    let conn = open_test_db(&db_dir);
    seed_data(&conn, &db_dir);
    let stages = std::sync::Mutex::new(Vec::new());
    export_backup(&conn, &db_dir, &zip_path, Some(&|stage, _d, _t| {
        let mut g = stages.lock().unwrap();
        if g.last() != Some(&stage.to_string()) {
            g.push(stage.to_string());
        }
    }))
    .unwrap();
    let stages = stages.into_inner().unwrap();
    assert!(stages.contains(&"snapshot".to_string()));
    assert!(stages.contains(&"photos".to_string()));
    assert!(stages.contains(&"finalize".to_string()));
}

// ---------------------------------------------------------------------------
// 往返一致性（导出 → 清空 → 恢复 → 逐表比对）
// ---------------------------------------------------------------------------

#[test]
fn export_restore_roundtrip_preserves_everything() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");

    // 造数据并记录期望值。
    let seeded = {
        let conn = open_test_db(&db_dir);
        let seeded = seed_data(&conn, &db_dir);
        assert_eq!(units::list_units(&conn).unwrap().len(), 1);
        assert_eq!(boxes::list_boxes(&conn, seeded.unit_id).unwrap().len(), 1);
        assert_eq!(items::list_items(&conn, seeded.box_id).unwrap().len(), 2);
        assert_eq!(categories::list_categories(&conn).unwrap().len(), 2);

        export_backup(&conn, &db_dir, &zip_path, None).unwrap();
        seeded
    };

    // 清空正式目录后恢复。
    fs::remove_dir_all(&db_dir).unwrap();
    fs::create_dir_all(&db_dir).unwrap();
    let stats = restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap();
    assert!(stats.files_extracted >= 3); // db + 2 照片 + manifest
    assert!(stats.has_photos);

    // 逐表比对。
    let conn = Connection::open(db_dir.join("findit.db")).unwrap();
    let after_units = units::list_units(&conn).unwrap();
    assert_eq!(after_units.len(), 1);
    assert_eq!(after_units[0].name, "客厅柜子");
    assert_eq!(after_units[0].description, "沙发旁边的柜子");

    let after_boxes = boxes::list_boxes(&conn, seeded.unit_id).unwrap();
    assert_eq!(after_boxes.len(), 1);
    assert_eq!(after_boxes[0].slug, seeded.box_slug);
    assert_eq!(after_boxes[0].name, "冬装箱");

    let after_items = items::list_items(&conn, seeded.box_id).unwrap();
    assert_eq!(after_items.len(), 2);
    let jacket = after_items.iter().find(|i| i.name == "羽绒服").unwrap();
    assert_eq!(jacket.id, seeded.item_ids[0]);
    assert_eq!(jacket.categories, vec!["衣物".to_string()]);
    assert_eq!(jacket.photo_path.as_deref(), Some(seeded.photo_main.as_str()));
    let warmer = after_items.iter().find(|i| i.name == "暖手宝").unwrap();
    assert_eq!(warmer.id, seeded.item_ids[1]);
    assert_eq!(warmer.quantity, 3);
    assert!(warmer.categories.is_empty());

    // 向量 BLOB 往返。
    let blob: Vec<u8> = conn
        .query_row(
            "SELECT embedding FROM items WHERE id = ?1",
            [seeded.item_ids[1]],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(blob, vec![1u8, 2, 3, 4]);

    let after_cats: Vec<String> = categories::list_categories(&conn)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(after_cats, vec!["电子产品".to_string(), "衣物".to_string()]);

    assert_eq!(
        settings::get_setting(&conn, "ollama_url").unwrap(),
        Some("http://192.168.1.10:11434".to_string())
    );
    assert_eq!(
        settings::get_setting(&conn, "chat_model").unwrap(),
        Some("qwen3:4b".to_string())
    );

    // 照片逐字节一致。
    let photos = db_dir.join("photos");
    let restored_main = fs::read(photos.join(&seeded.photo_main)).unwrap();
    assert_eq!(restored_main, seeded.photo_bytes_main);
    let thumb_name = format!(
        "{}_thumb.jpg",
        seeded.photo_main.trim_end_matches(".jpg")
    );
    let restored_thumb = fs::read(photos.join(&thumb_name)).unwrap();
    assert_eq!(restored_thumb, seeded.photo_bytes_thumb);
}

#[test]
fn restore_keeps_one_old_backup_copy_inside_data_dir() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");

    let conn = open_test_db(&db_dir);
    units::create_unit(&conn, "单元A", "").unwrap();
    export_backup(&conn, &db_dir, &zip_path, None).unwrap();
    drop(conn);

    // 连续恢复两次：旧数据副本应收敛为数据目录内的一份隐藏副本（S-L1：
    // 移入 .old-data-* 使其处于平台云备份排除范围，且不无限堆积）。
    restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap();
    restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap();

    // 数据目录旁不再有裸奔的 .backup-* 明文副本。
    let parent = db_dir.parent().unwrap();
    let sibling_backups: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("data.backup-")
        })
        .collect();
    assert!(sibling_backups.is_empty(), "旧数据副本不应留在数据目录旁：{sibling_backups:?}");

    // 数据目录内应恰好保留一份隐藏旧副本。
    let hidden: Vec<_> = fs::read_dir(&db_dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with(".old-data-")
                && e.path().is_dir()
        })
        .collect();
    assert_eq!(hidden.len(), 1, "应只保留一份隐藏旧数据副本");
}

// ---------------------------------------------------------------------------
// WAL 场景：写入后不做 checkpoint 直接导出
// ---------------------------------------------------------------------------

#[test]
fn wal_uncheckpointed_data_survives_export_restore() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");

    {
        let conn = open_test_db(&db_dir);
        units::create_unit(&conn, "WAL 单元", "尚未 checkpoint 的数据").unwrap();
        // 不做 wal_checkpoint，直接导出（导出内部会自行 checkpoint）。
        export_backup(&conn, &db_dir, &zip_path, None).unwrap();
    }

    let db_dir_b = root.join("data_b");
    fs::create_dir_all(&db_dir_b).unwrap();
    restore_backup(&zip_path, &db_dir_b, &DEFAULT_RESTORE_LIMITS, None).unwrap();

    let conn = Connection::open(db_dir_b.join("findit.db")).unwrap();
    let found = units::list_units(&conn).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "WAL 单元");
}

// ---------------------------------------------------------------------------
// 恢复校验链
// ---------------------------------------------------------------------------

#[test]
fn restore_rejects_non_zip_extension() {
    let root = temp_root();
    let path = root.join("backup.txt");
    fs::write(&path, b"whatever").unwrap();
    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let err = restore_backup(&path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("zip"));
}

#[test]
fn restore_rejects_garbage_with_zip_extension() {
    let root = temp_root();
    let path = root.join("backup.zip");
    fs::write(&path, b"this is definitely not a zip archive").unwrap();
    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let err = restore_backup(&path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("无法打开"));
}

#[test]
fn restore_rejects_oversized_zip_with_small_limit() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");
    {
        let conn = open_test_db(&db_dir);
        units::create_unit(&conn, "单元", "").unwrap();
        export_backup(&conn, &db_dir, &zip_path, None).unwrap();
    }
    // 用极小的压缩上限参数化（生产上限为 1GB）。
    let limits = RestoreLimits {
        max_zip_bytes: 10,
        ..DEFAULT_RESTORE_LIMITS
    };
    let err = restore_backup(&zip_path, &db_dir, &limits, None).unwrap_err();
    assert!(err.to_string().contains("过大"));
}

#[test]
fn restore_rejects_over_uncompressed_limit() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");
    {
        let conn = open_test_db(&db_dir);
        units::create_unit(&conn, "单元", "").unwrap();
        export_backup(&conn, &db_dir, &zip_path, None).unwrap();
    }
    let limits = RestoreLimits {
        max_uncompressed_bytes: 100, // 合法数据库也超过该值
        ..DEFAULT_RESTORE_LIMITS
    };
    let err = restore_backup(&zip_path, &db_dir, &limits, None).unwrap_err();
    assert!(err.to_string().contains("上限"));
}

#[test]
fn restore_rejects_too_many_entries() {
    // S-L2：海量微小条目 DoS 防护 —— 条目数超过上限时直接拒绝。
    let root = temp_root();
    let zip_path = root.join("many_entries.zip");
    // 40 个微小条目（超出测试用上限 30）。
    let entries: Vec<(String, &[u8])> = (0..40)
        .map(|i| (format!("files/f{i}.txt"), b"x".as_slice()))
        .collect();
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), *b))
        .collect();
    write_test_zip(&zip_path, &refs);

    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let limits = RestoreLimits {
        max_entry_count: 30,
        ..DEFAULT_RESTORE_LIMITS
    };
    let err = restore_backup(&zip_path, &db_dir, &limits, None).unwrap_err();
    assert!(err.to_string().contains("条目数过多"));
    assert_no_backup_residue(&db_dir);
    // 正常备份（条目数在内）不受影响。
    let limits_ok = RestoreLimits {
        max_entry_count: 1000,
        ..DEFAULT_RESTORE_LIMITS
    };
    let err2 = restore_backup(&zip_path, &db_dir, &limits_ok, None).unwrap_err();
    assert!(err2.to_string().contains("findit.db"), "超限放行后应走到数据库缺失校验");
}

#[test]
fn restore_rejects_zip_slip_entry() {
    let root = temp_root();
    let zip_path = root.join("evil.zip");
    write_test_zip(
        &zip_path,
        &[
            ("findit.db", b"SQLite format 3 placeholder"),
            ("../evil.txt", b"pwned"),
        ],
    );
    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let err = restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("非法路径"));
    assert!(!root.join("evil.txt").exists(), "zip-slip 文件不得落盘");
    assert_no_backup_residue(&db_dir);
}

#[test]
fn restore_rejects_zip_bomb_high_ratio() {
    let root = temp_root();
    let zip_path = root.join("bomb.zip");
    // 20MB 零字节经 deflate 后约 20KB，压缩比约 1000 > 100。
    let zeros = vec![0u8; 20 * 1024 * 1024];
    write_test_zip(&zip_path, &[("findit.db", &zeros)]);
    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let err = restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("压缩比"));
    assert_no_backup_residue(&db_dir);
}

#[test]
fn restore_rejects_missing_findit_db() {
    let root = temp_root();
    let zip_path = root.join("no_db.zip");
    write_test_zip(&zip_path, &[("photos/a.jpg", b"fake")]);
    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let err = restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("findit.db"));
}

#[test]
fn restore_rejects_corrupt_db_and_preserves_live_data() {
    let root = temp_root();
    let db_dir = root.join("data");

    // 正式库里先有真实数据。
    let conn = open_test_db(&db_dir);
    units::create_unit(&conn, "幸存单元", "恢复失败后必须还在").unwrap();
    drop(conn);

    // 备份里的 findit.db 不是 SQLite 内容（附合法 manifest，验证到数据库层才拒绝）。
    let zip_path = root.join("bad.zip");
    write_test_zip(
        &zip_path,
        &[
            ("findit.db", b"not a sqlite database at all"),
            ("manifest.json", VALID_MANIFEST.as_bytes()),
        ],
    );

    let err = restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("损坏"));

    // 回滚验证：正式目录完好、无副本与临时残留。
    let conn = Connection::open(db_dir.join("findit.db")).unwrap();
    let found = units::list_units(&conn).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "幸存单元");
    drop(conn);
    assert_no_backup_residue(&db_dir);
}

#[test]
fn export_scrubs_api_key_from_snapshot() {
    let root = temp_root();
    let db_dir = root.join("data");
    let zip_path = root.join("backup.zip");
    {
        let conn = open_test_db(&db_dir);
        seed_data(&conn, &db_dir);
        // 对话 Key 与向量独立 Key 都属于密钥类设置，导出时必须全部剔除。
        settings::set_setting(&conn, "ai_api_key", "sk-secret-123").unwrap();
        settings::set_setting(&conn, "ai_embed_api_key", "sk-embed-456").unwrap();
        export_backup(&conn, &db_dir, &zip_path, None).unwrap();
    }

    // 从 zip 里取出 findit.db 验证密钥已被剔除。
    let file = File::open(&zip_path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let mut db_bytes = Vec::new();
    archive.by_name("findit.db").unwrap().read_to_end(&mut db_bytes).unwrap();
    let snapshot_path = root.join("snapshot-check.db");
    fs::write(&snapshot_path, &db_bytes).unwrap();
    let conn = Connection::open(&snapshot_path).unwrap();
    // 对话 API Key 必须为空。
    let chat: String = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'ai_api_key'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(chat.is_empty(), "备份快照中的对话 API Key 必须为空");
    // 向量 API Key 必须为空（S-H3 回归：此前遗漏 ai_embed_api_key）。
    let embed: String = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'ai_embed_api_key'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(embed.is_empty(), "备份快照中的向量 API Key 必须为空");
    // 其它设置不受影响。
    let other: String = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'ollama_url'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other, "http://192.168.1.10:11434");
}

// ---------------------------------------------------------------------------
// 恢复校验链：schema / manifest
// ---------------------------------------------------------------------------

/// 构造合法备份的 zip 内 db 字节（真实 SQLite 库）。
fn valid_backup_db_bytes(root: &Path) -> Vec<u8> {
    let db_dir = root.join("src");
    let conn = open_test_db(&db_dir);
    units::create_unit(&conn, "单元", "").unwrap();
    drop(conn);
    fs::read(db_dir.join("findit.db")).unwrap()
}

const VALID_MANIFEST: &str = r#"{"app":"findit","format_version":1}"#;

#[test]
fn restore_rejects_missing_manifest() {
    let root = temp_root();
    let zip_path = root.join("no_manifest.zip");
    let db = valid_backup_db_bytes(&root);
    write_test_zip(&zip_path, &[("findit.db", &db)]);
    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let err = restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("manifest.json"));
    assert_no_backup_residue(&db_dir);
}

#[test]
fn restore_rejects_future_format_version() {
    let root = temp_root();
    let zip_path = root.join("future.zip");
    let db = valid_backup_db_bytes(&root);
    write_test_zip(
        &zip_path,
        &[
            ("findit.db", &db),
            ("manifest.json", b"{\"app\":\"findit\",\"format_version\":99}"),
        ],
    );
    let db_dir = root.join("data");
    fs::create_dir_all(&db_dir).unwrap();
    let err = restore_backup(&zip_path, &db_dir, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("格式版本过新"));
}

#[test]
fn restore_rejects_higher_user_version() {
    let root = temp_root();
    let db_dir = root.join("src");
    let conn = open_test_db(&db_dir);
    units::create_unit(&conn, "单元", "").unwrap();
    // 模拟来自更新版本应用的库。
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(conn);
    let db = fs::read(db_dir.join("findit.db")).unwrap();

    let zip_path = root.join("newer.zip");
    write_test_zip(&zip_path, &[("findit.db", &db), ("manifest.json", VALID_MANIFEST.as_bytes())]);
    let target = root.join("data");
    fs::create_dir_all(&target).unwrap();
    let err = restore_backup(&zip_path, &target, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("版本过新"));
    assert_no_backup_residue(&target);
}

#[test]
fn restore_rejects_missing_required_table() {
    let root = temp_root();
    let db_dir = root.join("src");
    let conn = open_test_db(&db_dir);
    units::create_unit(&conn, "单元", "").unwrap();
    conn.execute_batch("DROP TABLE app_settings").unwrap();
    drop(conn);
    let db = fs::read(db_dir.join("findit.db")).unwrap();

    let zip_path = root.join("no_table.zip");
    write_test_zip(&zip_path, &[("findit.db", &db), ("manifest.json", VALID_MANIFEST.as_bytes())]);
    let target = root.join("data");
    fs::create_dir_all(&target).unwrap();
    let err = restore_backup(&zip_path, &target, &DEFAULT_RESTORE_LIMITS, None).unwrap_err();
    assert!(err.to_string().contains("缺少必需表"));
}

#[test]
fn sanitize_entry_name_rules() {
    use super::restore::sanitize_entry_name;
    assert_eq!(sanitize_entry_name("findit.db").unwrap(), Some("findit.db".into()));
    assert_eq!(
        sanitize_entry_name("photos/a.jpg").unwrap(),
        Some("photos/a.jpg".into())
    );
    assert_eq!(sanitize_entry_name("photos/").unwrap(), None); // 目录条目
    assert!(sanitize_entry_name("../evil").is_err());
    assert!(sanitize_entry_name("a/../../etc/passwd").is_err());
    assert!(sanitize_entry_name("/abs/path").unwrap() == Some("abs/path".into())); // 前导 / 被剥离
    assert!(sanitize_entry_name("C:\\windows\\x").is_err());
    assert!(sanitize_entry_name("a/./b").is_err());
}
