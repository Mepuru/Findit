//! 存储单元（storage_units）CRUD。

use rusqlite::{params, Connection};

use crate::api::model::Unit;
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo::{is_unique_violation, validate_name};

const ENTITY: &str = "存储单元";

fn row_to_unit(id: i64, name: String, description: String, box_count: i64) -> Unit {
    Unit {
        id,
        name,
        description,
        box_count,
    }
}

/// 创建存储单元。名称重名返回 [`FinditError::DuplicateName`]。
pub fn create_unit(conn: &Connection, name: &str, description: &str) -> FinditResult<Unit> {
    let name = validate_name(ENTITY, name)?;
    let description = description.trim().to_string();
    let result = conn.execute(
        "INSERT INTO storage_units (name, description) VALUES (?1, ?2)",
        params![name, description],
    );
    if let Err(e) = result {
        if is_unique_violation(&e) {
            return Err(FinditError::DuplicateName {
                entity: ENTITY.to_string(),
                name,
            });
        }
        return Err(e.into());
    }
    let id = conn.last_insert_rowid();
    Ok(row_to_unit(id, name, description, 0))
}

/// 列出全部存储单元（附带每个单元下的箱子数），按名称排序。
pub fn list_units(conn: &Connection) -> FinditResult<Vec<Unit>> {
    let mut stmt = conn.prepare(
        "SELECT u.id, u.name, u.description, COUNT(b.id) \
         FROM storage_units u \
         LEFT JOIN storage_boxes b ON b.unit_id = u.id \
         GROUP BY u.id \
         ORDER BY u.name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(row_to_unit(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
        ))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// 按 id 读取单个存储单元。
pub fn get_unit(conn: &Connection, id: i64) -> FinditResult<Unit> {
    let result = conn.query_row(
        "SELECT u.id, u.name, u.description, COUNT(b.id) \
         FROM storage_units u \
         LEFT JOIN storage_boxes b ON b.unit_id = u.id \
         WHERE u.id = ?1 \
         GROUP BY u.id",
        params![id],
        |row| Ok(row_to_unit(row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    );
    match result {
        Ok(unit) => Ok(unit),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={id}"),
        }),
        Err(e) => Err(e.into()),
    }
}

/// 更新存储单元；`None` 字段表示不更新。
pub fn update_unit(
    conn: &Connection,
    id: i64,
    name: Option<String>,
    description: Option<String>,
) -> FinditResult<Unit> {
    // 确保存在
    get_unit(conn, id)?;

    let mut sets: Vec<String> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(raw) = name {
        let name = validate_name(ENTITY, &raw)?;
        // 重名检查（排除自身）
        let dup: i64 = conn.query_row(
            "SELECT COUNT(*) FROM storage_units WHERE name = ?1 AND id != ?2",
            params![name, id],
            |row| row.get(0),
        )?;
        if dup > 0 {
            return Err(FinditError::DuplicateName {
                entity: ENTITY.to_string(),
                name,
            });
        }
        sets.push("name = ?".to_string());
        values.push(Box::new(name));
    }
    if let Some(raw) = description {
        sets.push("description = ?".to_string());
        values.push(Box::new(raw.trim().to_string()));
    }

    if !sets.is_empty() {
        values.push(Box::new(id));
        let sql = format!(
            "UPDATE storage_units SET {} WHERE id = ?",
            sets.join(", ")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        conn.execute(&sql, params.as_slice())?;
    }

    get_unit(conn, id)
}

/// 删除存储单元，级联删除其下所有收纳箱、物品及物品分类关联（显式事务）。
/// 事务提交成功后尽力清理子树物品的照片文件（失败仅记日志不阻断）。
pub fn delete_unit(conn: &Connection, id: i64) -> FinditResult<()> {
    let tx = conn.unchecked_transaction()?;

    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM storage_units WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={id}"),
        });
    }

    // 删除前收集子树物品的照片路径，供提交后清理文件。
    let photos: Vec<String> = tx
        .prepare(
            "SELECT photo_path FROM items \
             WHERE box_id IN (SELECT id FROM storage_boxes WHERE unit_id = ?1) \
             AND photo_path IS NOT NULL",
        )?
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    // 该单元下的全部箱子（先删子表，避免外键约束下删父表失败）
    let box_ids: Vec<i64> = tx
        .prepare("SELECT id FROM storage_boxes WHERE unit_id = ?1")?
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for box_id in &box_ids {
        // 箱内物品的分类关联
        tx.execute(
            "DELETE FROM item_categories WHERE item_id IN \
             (SELECT id FROM items WHERE box_id = ?1)",
            params![box_id],
        )?;
        tx.execute("DELETE FROM items WHERE box_id = ?1", params![box_id])?;
    }
    tx.execute("DELETE FROM storage_boxes WHERE unit_id = ?1", params![id])?;
    tx.execute("DELETE FROM storage_units WHERE id = ?1", params![id])?;

    tx.commit()?;
    crate::core::photo::cleanup_photo_paths_best_effort(&photos);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn create_and_get_unit() {
        let conn = setup();
        let unit = create_unit(&conn, "客厅柜子", "靠窗的那个").unwrap();
        assert_eq!(unit.name, "客厅柜子");
        assert_eq!(unit.description, "靠窗的那个");
        assert_eq!(unit.box_count, 0);

        let fetched = get_unit(&conn, unit.id).unwrap();
        assert_eq!(fetched, unit);
    }

    #[test]
    fn create_trims_and_rejects_empty_name() {
        let conn = setup();
        let unit = create_unit(&conn, "  书房  ", "").unwrap();
        assert_eq!(unit.name, "书房");
        assert!(matches!(
            create_unit(&conn, "   ", ""),
            Err(FinditError::Validation(_))
        ));
    }

    #[test]
    fn duplicate_name_rejected() {
        let conn = setup();
        create_unit(&conn, "储藏室", "").unwrap();
        let err = create_unit(&conn, "储藏室", "").unwrap_err();
        assert!(matches!(err, FinditError::DuplicateName { .. }));
    }

    #[test]
    fn list_units_sorted_with_box_count() {
        let conn = setup();
        let b = create_unit(&conn, "B单元", "").unwrap();
        create_unit(&conn, "A单元", "").unwrap();
        conn.execute(
            "INSERT INTO storage_boxes (slug, name, unit_id, created_at, updated_at) \
             VALUES ('s1', '箱1', ?1, '', '')",
            params![b.id],
        )
        .unwrap();
        let units = list_units(&conn).unwrap();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].name, "A单元");
        assert_eq!(units[1].name, "B单元");
        assert_eq!(units[1].box_count, 1);
    }

    #[test]
    fn update_partial_fields() {
        let conn = setup();
        let unit = create_unit(&conn, "原名", "原描述").unwrap();

        // 只改描述
        let updated = update_unit(&conn, unit.id, None, Some("新描述".into())).unwrap();
        assert_eq!(updated.name, "原名");
        assert_eq!(updated.description, "新描述");

        // 只改名称
        let updated = update_unit(&conn, unit.id, Some("新名".into()), None).unwrap();
        assert_eq!(updated.name, "新名");
        assert_eq!(updated.description, "新描述");

        // 全 None 不变
        let updated = update_unit(&conn, unit.id, None, None).unwrap();
        assert_eq!(updated.name, "新名");
    }

    #[test]
    fn update_name_conflict_rejected() {
        let conn = setup();
        let a = create_unit(&conn, "甲", "").unwrap();
        create_unit(&conn, "乙", "").unwrap();
        let err = update_unit(&conn, a.id, Some("乙".into()), None).unwrap_err();
        assert!(matches!(err, FinditError::DuplicateName { .. }));

        // 改名与自身相同不算冲突
        let ok = update_unit(&conn, a.id, Some("甲".into()), None).unwrap();
        assert_eq!(ok.name, "甲");
    }

    #[test]
    fn get_missing_unit_not_found() {
        let conn = setup();
        assert!(matches!(
            get_unit(&conn, 999),
            Err(FinditError::NotFound { .. })
        ));
        assert!(matches!(
            delete_unit(&conn, 999),
            Err(FinditError::NotFound { .. })
        ));
    }

    #[test]
    fn delete_unit_cascades_boxes_items_and_links() {
        let conn = setup();
        let unit = create_unit(&conn, "柜子", "").unwrap();
        conn.execute(
            "INSERT INTO storage_boxes (slug, name, unit_id, created_at, updated_at) \
             VALUES ('s1', '箱1', ?1, '', '')",
            params![unit.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO items (name, box_id) SELECT '手电', id FROM storage_boxes",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO categories (name) VALUES ('工具')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO item_categories (item_id, category_id) \
             SELECT i.id, c.id FROM items i, categories c",
            [],
        )
        .unwrap();

        delete_unit(&conn, unit.id).unwrap();

        for (sql, expect) in [
            ("SELECT COUNT(*) FROM storage_units", 0),
            ("SELECT COUNT(*) FROM storage_boxes", 0),
            ("SELECT COUNT(*) FROM items", 0),
            ("SELECT COUNT(*) FROM item_categories", 0),
            // 分类本身不随单元删除
            ("SELECT COUNT(*) FROM categories", 1),
        ] {
            let n: i64 = conn.query_row(sql, [], |r| r.get(0)).unwrap();
            assert_eq!(n, expect, "检查 {sql}");
        }
    }
}
