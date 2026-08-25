//! 物品（items）CRUD，含物品-分类多对多关联。

use rusqlite::{params, Connection};

use crate::api::model::Item;
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo::{load_item_categories, validate_name};
use crate::core::util::now_iso8601;

const ENTITY: &str = "物品";

struct ItemRow {
    id: i64,
    name: String,
    description: String,
    quantity: i64,
    photo_path: Option<String>,
    box_id: i64,
    created_at: String,
    updated_at: String,
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        quantity: row.get(3)?,
        photo_path: row.get(4)?,
        box_id: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn finish_item(conn: &Connection, row: ItemRow) -> FinditResult<Item> {
    let mut cats = load_item_categories(conn, &[row.id])?
        .remove(&row.id)
        .unwrap_or_default();
    cats.sort();
    Ok(Item {
        id: row.id,
        name: row.name,
        description: row.description,
        quantity: row.quantity,
        photo_path: row.photo_path,
        box_id: row.box_id,
        categories: cats,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn require_box(conn: &Connection, box_id: i64) -> FinditResult<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM storage_boxes WHERE id = ?1",
        params![box_id],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(FinditError::NotFound {
            entity: "收纳箱".to_string(),
            hint: format!("id={box_id}"),
        });
    }
    Ok(())
}

fn validate_category_ids(conn: &Connection, category_ids: &[i64]) -> FinditResult<Vec<i64>> {
    let mut ids: Vec<i64> = category_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    for id in &ids {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM categories WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(FinditError::NotFound {
                entity: "分类".to_string(),
                hint: format!("id={id}"),
            });
        }
    }
    Ok(ids)
}

fn replace_item_categories(conn: &Connection, item_id: i64, category_ids: &[i64]) -> FinditResult<()> {
    conn.execute("DELETE FROM item_categories WHERE item_id = ?1", params![item_id])?;
    for cat_id in category_ids {
        conn.execute(
            "INSERT OR IGNORE INTO item_categories (item_id, category_id) VALUES (?1, ?2)",
            params![item_id, cat_id],
        )?;
    }
    Ok(())
}

/// 创建物品。`category_ids` 为分类 id 列表（可空）。
pub fn create_item(
    conn: &Connection,
    box_id: i64,
    name: &str,
    description: &str,
    quantity: i64,
    category_ids: &[i64],
) -> FinditResult<Item> {
    let name = validate_name(ENTITY, name)?;
    let description = description.trim().to_string();
    if quantity < 1 {
        return Err(FinditError::Validation("物品数量必须大于等于 1".to_string()));
    }
    require_box(conn, box_id)?;
    let category_ids = validate_category_ids(conn, category_ids)?;

    let now = now_iso8601();
    conn.execute(
        "INSERT INTO items (name, description, quantity, box_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![name, description, quantity, box_id, now, now],
    )?;
    let id = conn.last_insert_rowid();
    replace_item_categories(conn, id, &category_ids)?;

    get_item(conn, id)
}

const SELECT_COLS: &str = "SELECT id, name, description, quantity, photo_path, box_id, \
                           COALESCE(created_at, ''), COALESCE(updated_at, '') FROM items";

/// 列出某箱内全部物品（附带每个物品的分类列表），按名称排序。
pub fn list_items(conn: &Connection, box_id: i64) -> FinditResult<Vec<Item>> {
    let sql = format!("{SELECT_COLS} WHERE box_id = ?1 ORDER BY name");
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<ItemRow> = stmt
        .query_map(params![box_id], row_to_item)?
        .collect::<Result<Vec<_>, _>>()?;

    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let cats = load_item_categories(conn, &ids)?;
    rows.into_iter()
        .map(|row| {
            let mut categories = cats.get(&row.id).cloned().unwrap_or_default();
            categories.sort();
            Ok(Item {
                id: row.id,
                name: row.name,
                description: row.description,
                quantity: row.quantity,
                photo_path: row.photo_path,
                box_id: row.box_id,
                categories,
                created_at: row.created_at,
                updated_at: row.updated_at,
            })
        })
        .collect::<FinditResult<Vec<Item>>>()
}

/// 按 id 读取物品。
pub fn get_item(conn: &Connection, id: i64) -> FinditResult<Item> {
    let sql = format!("{SELECT_COLS} WHERE id = ?1");
    match conn.query_row(&sql, params![id], row_to_item) {
        Ok(row) => finish_item(conn, row),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={id}"),
        }),
        Err(e) => Err(e.into()),
    }
}

/// 更新物品；`None` 字段表示不更新；`category_ids` 为 `Some` 时整体替换。
#[allow(clippy::too_many_arguments)]
pub fn update_item(
    conn: &Connection,
    id: i64,
    name: Option<String>,
    description: Option<String>,
    quantity: Option<i64>,
    box_id: Option<i64>,
    category_ids: Option<Vec<i64>>,
) -> FinditResult<Item> {
    get_item(conn, id)?;

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
    if let Some(quantity) = quantity {
        if quantity < 1 {
            return Err(FinditError::Validation("物品数量必须大于等于 1".to_string()));
        }
        sets.push("quantity = ?".to_string());
        values.push(Box::new(quantity));
    }
    if let Some(box_id) = box_id {
        require_box(conn, box_id)?;
        sets.push("box_id = ?".to_string());
        values.push(Box::new(box_id));
    }

    if !sets.is_empty() {
        sets.push("updated_at = ?".to_string());
        values.push(Box::new(now_iso8601()));
        values.push(Box::new(id));
        let sql = format!("UPDATE items SET {} WHERE id = ?", sets.join(", "));
        let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        conn.execute(&sql, params.as_slice())?;
    }

    if let Some(category_ids) = category_ids {
        let category_ids = validate_category_ids(conn, &category_ids)?;
        replace_item_categories(conn, id, &category_ids)?;
    }

    get_item(conn, id)
}

/// 更新物品的照片路径；`None` 表示清空照片。物品不存在时报错。
pub fn set_item_photo_path(
    conn: &Connection,
    id: i64,
    photo_path: Option<&str>,
) -> FinditResult<()> {
    get_item(conn, id)?;
    conn.execute(
        "UPDATE items SET photo_path = ?1, updated_at = ?2 WHERE id = ?3",
        params![photo_path, now_iso8601(), id],
    )?;
    Ok(())
}

/// 删除物品及其分类关联（显式事务）。
pub fn delete_item(conn: &Connection, id: i64) -> FinditResult<()> {
    let tx = conn.unchecked_transaction()?;
    let affected = tx.execute("DELETE FROM items WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={id}"),
        });
    }
    tx.execute("DELETE FROM item_categories WHERE item_id = ?1", params![id])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::{boxes, categories, units};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn unit_and_box(conn: &Connection) -> i64 {
        let unit = units::create_unit(conn, "柜子", "").unwrap();
        boxes::create_box(conn, unit.id, "箱子", "").unwrap().id
    }

    #[test]
    fn create_item_with_categories() {
        let conn = setup();
        let box_id = unit_and_box(&conn);
        let c1 = categories::create_category(&conn, "电子").unwrap();
        let c2 = categories::create_category(&conn, "线材").unwrap();

        let item = create_item(&conn, box_id, "充电线", "Type-C", 3, &[c1.id, c2.id, c1.id]).unwrap();
        assert_eq!(item.name, "充电线");
        assert_eq!(item.quantity, 3);
        assert_eq!(item.categories, vec!["电子".to_string(), "线材".to_string()]);
        assert!(item.photo_path.is_none());
    }

    #[test]
    fn create_item_validations() {
        let conn = setup();
        let box_id = unit_and_box(&conn);
        assert!(matches!(
            create_item(&conn, box_id, "", "", 1, &[]),
            Err(FinditError::Validation(_))
        ));
        assert!(matches!(
            create_item(&conn, box_id, "物", "", 0, &[]),
            Err(FinditError::Validation(_))
        ));
        assert!(matches!(
            create_item(&conn, 999, "物", "", 1, &[]),
            Err(FinditError::NotFound { .. })
        ));
        assert!(matches!(
            create_item(&conn, box_id, "物", "", 1, &[42]),
            Err(FinditError::NotFound { .. })
        ));
    }

    #[test]
    fn list_items_includes_categories() {
        let conn = setup();
        let box_id = unit_and_box(&conn);
        let cat = categories::create_category(&conn, "工具").unwrap();
        create_item(&conn, box_id, "锤子", "", 1, &[cat.id]).unwrap();
        create_item(&conn, box_id, "螺丝刀", "", 2, &[]).unwrap();

        let items = list_items(&conn, box_id).unwrap();
        assert_eq!(items.len(), 2);
        let hammer = items.iter().find(|i| i.name == "锤子").unwrap();
        assert_eq!(hammer.categories, vec!["工具".to_string()]);
    }

    #[test]
    fn update_item_partial_and_categories() {
        let conn = setup();
        let box_id = unit_and_box(&conn);
        let c1 = categories::create_category(&conn, "A").unwrap();
        let c2 = categories::create_category(&conn, "B").unwrap();
        let item = create_item(&conn, box_id, "物", "旧", 1, &[c1.id]).unwrap();

        // 只改数量
        let updated = update_item(&conn, item.id, None, None, Some(5), None, None).unwrap();
        assert_eq!(updated.quantity, 5);
        assert_eq!(updated.categories, vec!["A".to_string()]);

        // 替换分类为空列表
        let updated = update_item(&conn, item.id, None, None, None, None, Some(vec![])).unwrap();
        assert!(updated.categories.is_empty());

        // 整体替换分类
        let updated =
            update_item(&conn, item.id, Some("新物".into()), None, None, None, Some(vec![c2.id]))
                .unwrap();
        assert_eq!(updated.name, "新物");
        assert_eq!(updated.categories, vec!["B".to_string()]);
    }

    #[test]
    fn update_item_move_box() {
        let conn = setup();
        let box_id = unit_and_box(&conn);
        let unit_id: i64 = conn
            .query_row("SELECT id FROM storage_units", [], |r| r.get(0))
            .unwrap();
        let other_box = boxes::create_box(&conn, unit_id, "另一箱", "").unwrap();
        let item = create_item(&conn, box_id, "物", "", 1, &[]).unwrap();

        let moved = update_item(&conn, item.id, None, None, None, Some(other_box.id), None).unwrap();
        assert_eq!(moved.box_id, other_box.id);

        assert!(matches!(
            update_item(&conn, item.id, None, None, None, Some(999), None),
            Err(FinditError::NotFound { .. })
        ));
    }

    #[test]
    fn delete_item_removes_category_links() {
        let conn = setup();
        let box_id = unit_and_box(&conn);
        let cat = categories::create_category(&conn, "A").unwrap();
        let item = create_item(&conn, box_id, "物", "", 1, &[cat.id]).unwrap();

        delete_item(&conn, item.id).unwrap();
        let links: i64 = conn
            .query_row("SELECT COUNT(*) FROM item_categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(links, 0);
        // 分类本身保留
        let cats: i64 = conn
            .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cats, 1);
        assert!(matches!(get_item(&conn, item.id), Err(FinditError::NotFound { .. })));
    }
}
