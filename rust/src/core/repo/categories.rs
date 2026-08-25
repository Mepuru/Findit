//! 分类（categories）CRUD。删除分类只解除与物品的关联，不删除物品。

use rusqlite::{params, Connection};

use crate::api::model::Category;
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo::{is_unique_violation, validate_name};

const ENTITY: &str = "分类";

/// 创建分类。重名返回 [`FinditError::DuplicateName`]。
pub fn create_category(conn: &Connection, name: &str) -> FinditResult<Category> {
    let name = validate_name(ENTITY, name)?;
    let result = conn.execute("INSERT INTO categories (name) VALUES (?1)", params![name]);
    if let Err(e) = result {
        if is_unique_violation(&e) {
            return Err(FinditError::DuplicateName {
                entity: ENTITY.to_string(),
                name,
            });
        }
        return Err(e.into());
    }
    Ok(Category {
        id: conn.last_insert_rowid(),
        name,
    })
}

/// 列出全部分类，按名称排序。
pub fn list_categories(conn: &Connection) -> FinditResult<Vec<Category>> {
    let mut stmt = conn.prepare("SELECT id, name FROM categories ORDER BY name")?;
    let rows = stmt.query_map([], |row| {
        Ok(Category {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// 重命名分类。
pub fn rename_category(conn: &Connection, id: i64, new_name: &str) -> FinditResult<Category> {
    let new_name = validate_name(ENTITY, new_name)?;

    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM categories WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={id}"),
        });
    }

    let dup: i64 = conn.query_row(
        "SELECT COUNT(*) FROM categories WHERE name = ?1 AND id != ?2",
        params![new_name, id],
        |row| row.get(0),
    )?;
    if dup > 0 {
        return Err(FinditError::DuplicateName {
            entity: ENTITY.to_string(),
            name: new_name,
        });
    }

    conn.execute("UPDATE categories SET name = ?1 WHERE id = ?2", params![new_name, id])?;
    Ok(Category { id, name: new_name })
}

/// 删除分类：只解除物品关联，不删除物品本身。
pub fn delete_category(conn: &Connection, id: i64) -> FinditResult<()> {
    let tx = conn.unchecked_transaction()?;
    let affected = tx.execute("DELETE FROM categories WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(FinditError::NotFound {
            entity: ENTITY.to_string(),
            hint: format!("id={id}"),
        });
    }
    tx.execute("DELETE FROM item_categories WHERE category_id = ?1", params![id])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::{boxes, items, units};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn create_list_rename_category() {
        let conn = setup();
        create_category(&conn, "工具").unwrap();
        create_category(&conn, "  电子  ").unwrap();
        assert!(matches!(
            create_category(&conn, "工具"),
            Err(FinditError::DuplicateName { .. })
        ));

        let cats = list_categories(&conn).unwrap();
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].name, "工具");
        assert_eq!(cats[1].name, "电子");

        let renamed = rename_category(&conn, cats[0].id, "五金").unwrap();
        assert_eq!(renamed.name, "五金");
        assert!(matches!(
            rename_category(&conn, cats[0].id, "电子"),
            Err(FinditError::DuplicateName { .. })
        ));
        assert!(matches!(
            rename_category(&conn, 999, "不存在"),
            Err(FinditError::NotFound { .. })
        ));
    }

    #[test]
    fn delete_category_unlinks_but_keeps_items() {
        let conn = setup();
        let unit = units::create_unit(&conn, "柜", "").unwrap();
        let bx = boxes::create_box(&conn, unit.id, "箱", "").unwrap();
        let cat = create_category(&conn, "工具").unwrap();
        items::create_item(&conn, bx.id, "锤子", "", 1, &[cat.id]).unwrap();

        delete_category(&conn, cat.id).unwrap();

        let item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(item_count, 1);
        let link_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM item_categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(link_count, 0);
        assert!(matches!(
            delete_category(&conn, cat.id),
            Err(FinditError::NotFound { .. })
        ));
    }
}
