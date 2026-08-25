//! 收纳箱（storage_boxes）CRUD。

use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::api::model::StorageBox as BoxModel;
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo::validate_name;
use crate::core::util::now_iso8601;

const ENTITY: &str = "收纳箱";

fn row_to_box(
    id: i64,
    slug: String,
    name: String,
    description: String,
    unit_id: i64,
    item_count: i64,
    created_at: String,
    updated_at: String,
) -> BoxModel {
    BoxModel {
        id,
        slug,
        name,
        description,
        unit_id,
        item_count,
        created_at,
        updated_at,
    }
}

/// 创建收纳箱；`slug` 为新生成的 UUID v4。
pub fn create_box(
    conn: &Connection,
    unit_id: i64,
    name: &str,
    description: &str,
) -> FinditResult<BoxModel> {
    let name = validate_name(ENTITY, name)?;
    let description = description.trim().to_string();

    // 所属单元必须存在
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM storage_units WHERE id = ?1",
        params![unit_id],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(FinditError::NotFound {
            entity: "存储单元".to_string(),
            hint: format!("id={unit_id}"),
        });
    }

    let slug = Uuid::new_v4().to_string();
    let now = now_iso8601();
    conn.execute(
        "INSERT INTO storage_boxes (slug, name, description, unit_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![slug, name, description, unit_id, now, now],
    )?;
    let id = conn.last_insert_rowid();
    Ok(row_to_box(id, slug, name, description, unit_id, 0, now.clone(), now))
}

/// 列出某单元下的全部收纳箱（附带每箱物品数），按名称排序。
pub fn list_boxes(conn: &Connection, unit_id: i64) -> FinditResult<Vec<BoxModel>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.slug, b.name, b.description, b.unit_id, COUNT(i.id), \
                b.created_at, b.updated_at \
         FROM storage_boxes b \
         LEFT JOIN items i ON i.box_id = b.id \
         WHERE b.unit_id = ?1 \
         GROUP BY b.id \
         ORDER BY b.name",
    )?;
    let rows = stmt.query_map(params![unit_id], |row| {
        Ok(row_to_box(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn query_box_by(conn: &Connection, where_clause: &str, param: i64) -> FinditResult<BoxModel> {
    let sql = format!(
        "SELECT b.id, b.slug, b.name, b.description, b.unit_id, COUNT(i.id), \
                b.created_at, b.updated_at \
         FROM storage_boxes b \
         LEFT JOIN items i ON i.box_id = b.id \
         WHERE {where_clause} \
         GROUP BY b.id"
    );
    match conn.query_row(&sql, params![param], |row| {
        Ok(row_to_box(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ))
    }) {
        Ok(b) => Ok(b),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={param}"),
        }),
        Err(e) => Err(e.into()),
    }
}

/// 按 id 读取收纳箱。
pub fn get_box(conn: &Connection, id: i64) -> FinditResult<BoxModel> {
    query_box_by(conn, "b.id = ?1", id)
}

/// 按 slug（二维码内容）读取收纳箱。
pub fn get_box_by_slug(conn: &Connection, slug: &str) -> FinditResult<BoxModel> {
    let sql = "SELECT b.id, b.slug, b.name, b.description, b.unit_id, COUNT(i.id), \
                      b.created_at, b.updated_at \
               FROM storage_boxes b \
               LEFT JOIN items i ON i.box_id = b.id \
               WHERE b.slug = ?1 \
               GROUP BY b.id";
    match conn.query_row(sql, params![slug.trim()], |row| {
        Ok(row_to_box(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ))
    }) {
        Ok(b) => Ok(b),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("slug={slug}"),
        }),
        Err(e) => Err(e.into()),
    }
}

/// 更新收纳箱；`None` 字段表示不更新。`unit_id` 可跨单元移动。
pub fn update_box(
    conn: &Connection,
    id: i64,
    name: Option<String>,
    description: Option<String>,
    unit_id: Option<i64>,
) -> FinditResult<BoxModel> {
    get_box(conn, id)?;

    let mut sets: Vec<String> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(raw) = name {
        let name = validate_name(ENTITY, &raw)?;
        sets.push("name = ?".to_string());
        values.push(Box::new(name));
    }
    if let Some(raw) = description {
        sets.push("description = ?".to_string());
        values.push(Box::new(raw.trim().to_string()));
    }
    if let Some(unit_id) = unit_id {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM storage_units WHERE id = ?1",
            params![unit_id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(FinditError::NotFound {
                entity: "存储单元".to_string(),
                hint: format!("id={unit_id}"),
            });
        }
        sets.push("unit_id = ?".to_string());
        values.push(Box::new(unit_id));
    }

    if !sets.is_empty() {
        sets.push("updated_at = ?".to_string());
        values.push(Box::new(now_iso8601()));
        values.push(Box::new(id));
        let sql = format!("UPDATE storage_boxes SET {} WHERE id = ?", sets.join(", "));
        let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        conn.execute(&sql, params.as_slice())?;
    }

    get_box(conn, id)
}

/// 删除收纳箱，级联删除箱内全部物品及其分类关联（显式事务）。
pub fn delete_box(conn: &Connection, id: i64) -> FinditResult<()> {
    let tx = conn.unchecked_transaction()?;

    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM storage_boxes WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={id}"),
        });
    }

    // 先删子表（分类关联、物品），再删箱子本身，避免外键约束冲突。
    tx.execute(
        "DELETE FROM item_categories WHERE item_id IN \
         (SELECT id FROM items WHERE box_id = ?1)",
        params![id],
    )?;
    tx.execute("DELETE FROM items WHERE box_id = ?1", params![id])?;
    tx.execute("DELETE FROM storage_boxes WHERE id = ?1", params![id])?;

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::units;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn create_box_generates_unique_slug() {
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let b1 = create_box(&conn, unit.id, "上层", "").unwrap();
        let b2 = create_box(&conn, unit.id, "下层", "").unwrap();
        assert_ne!(b1.slug, b2.slug);
        assert!(uuid::Uuid::parse_str(&b1.slug).is_ok());
        assert_eq!(b1.created_at, b1.updated_at);
    }

    #[test]
    fn create_box_requires_existing_unit() {
        let conn = setup();
        assert!(matches!(
            create_box(&conn, 42, "箱子", ""),
            Err(FinditError::NotFound { .. })
        ));
    }

    #[test]
    fn get_box_by_slug_roundtrip() {
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let b = create_box(&conn, unit.id, "箱子", "描述").unwrap();
        let by_slug = get_box_by_slug(&conn, &b.slug).unwrap();
        assert_eq!(by_slug.id, b.id);
        assert!(matches!(
            get_box_by_slug(&conn, "not-exist"),
            Err(FinditError::NotFound { .. })
        ));
    }

    #[test]
    fn list_boxes_with_item_count() {
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let b = create_box(&conn, unit.id, "箱子", "").unwrap();
        conn.execute(
            "INSERT INTO items (name, box_id) VALUES ('手电', ?1)",
            params![b.id],
        )
        .unwrap();
        let boxes = list_boxes(&conn, unit.id).unwrap();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].item_count, 1);
    }

    #[test]
    fn update_box_partial_and_move() {
        let conn = setup();
        let u1 = units::create_unit(&conn, "A", "").unwrap();
        let u2 = units::create_unit(&conn, "B", "").unwrap();
        let b = create_box(&conn, u1.id, "箱", "旧").unwrap();

        let updated = update_box(&conn, b.id, Some("新箱".into()), None, Some(u2.id)).unwrap();
        assert_eq!(updated.name, "新箱");
        assert_eq!(updated.description, "旧");
        assert_eq!(updated.unit_id, u2.id);
        assert!(updated.updated_at >= b.updated_at);

        assert!(matches!(
            update_box(&conn, b.id, None, None, Some(999)),
            Err(FinditError::NotFound { .. })
        ));
    }

    #[test]
    fn delete_box_cascades_items_and_links() {
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let b = create_box(&conn, unit.id, "箱", "").unwrap();
        conn.execute(
            "INSERT INTO items (name, box_id) VALUES ('手电', ?1)",
            params![b.id],
        )
        .unwrap();
        let item_id: i64 = conn
            .query_row("SELECT id FROM items", [], |r| r.get(0))
            .unwrap();
        conn.execute("INSERT INTO categories (name) VALUES ('工具')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO item_categories (item_id, category_id) \
             SELECT ?1, id FROM categories",
            params![item_id],
        )
        .unwrap();

        delete_box(&conn, b.id).unwrap();

        let items: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        let links: i64 = conn
            .query_row("SELECT COUNT(*) FROM item_categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(items, 0);
        assert_eq!(links, 0);
        assert!(matches!(get_box(&conn, b.id), Err(FinditError::NotFound { .. })));
    }
}
