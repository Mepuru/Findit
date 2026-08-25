//! 由 `PRAGMA user_version` 驱动的建表 / 升级迁移。
//!
//! 级联删除不依赖数据库外键约束，而是在业务层用显式事务实现，
//! 以保证行为与旧版 Python 实现的 ORM 级联一致。

use rusqlite::Connection;

use crate::core::error::FinditResult;

/// 当前 schema 版本号。
pub const CURRENT_VERSION: i64 = 1;

/// 按 `user_version` 顺序应用所有未执行的迁移。
///
/// 版本回写仅发生在升级路径（`version < CURRENT_VERSION`）；
/// 更高版本的库不会被静默改回低版本号（避免覆盖来自新版应用的备份）。
pub fn run_migrations(conn: &Connection) -> FinditResult<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if version < 1 {
        migrate_v1(conn)?;
    }

    if version < CURRENT_VERSION {
        conn.pragma_update(None, "user_version", CURRENT_VERSION)?;
    }
    Ok(())
}

/// v1：初始 schema —— 6 张表 + 3 个索引。
fn migrate_v1(conn: &Connection) -> FinditResult<()> {
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS storage_units (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS storage_boxes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    slug        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    unit_id     INTEGER NOT NULL REFERENCES storage_units(id),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    quantity    INTEGER NOT NULL DEFAULT 1,
    photo_path  TEXT,
    box_id      INTEGER NOT NULL REFERENCES storage_boxes(id),
    embedding   BLOB,
    created_at  TEXT,
    updated_at  TEXT
);

CREATE TABLE IF NOT EXISTS categories (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS item_categories (
    item_id     INTEGER NOT NULL,
    category_id INTEGER NOT NULL,
    PRIMARY KEY (item_id, category_id)
);

CREATE TABLE IF NOT EXISTS app_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_boxes_unit_id ON storage_boxes(unit_id);
CREATE INDEX IF NOT EXISTS idx_items_box_id ON items(box_id);
CREATE INDEX IF NOT EXISTS idx_item_categories_category_id ON item_categories(category_id);
",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_reaches_current_version() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_VERSION);

        // 6 张表全部存在
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('storage_units','storage_boxes','items','categories','item_categories','app_settings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 6);
    }

    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_VERSION);
    }

    #[test]
    fn higher_user_version_is_not_downgraded() {
        // 来自更新版本应用的库不应被静默改回当前版本号。
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", CURRENT_VERSION + 5)
            .unwrap();
        run_migrations(&conn).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_VERSION + 5);
    }
}
