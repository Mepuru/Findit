//! 关键词搜索：空白分词，词间 AND、字段间（名称/描述/分类名）OR。
//!
//! 拉丁词大小写不敏感：SQL 双侧使用 `lower()` 折叠（SQLite 原生
//! `lower()` 覆盖 ASCII；中文无大小写问题，直接按 LIKE 子串匹配）。

use rusqlite::{params_from_iter, Connection};

use crate::api::model::Item;
use crate::core::error::FinditResult;
use crate::core::repo::load_item_categories;

/// 一条关键词命中：物品完整信息 + 所在箱/单元名（便于结果定位）。
pub struct KeywordHit {
    pub item: Item,
    pub box_name: String,
    pub unit_name: String,
}

/// 空白分词，丢弃空词。
pub fn tokenize(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_string).collect()
}

/// 生成 `%token%` 形式的 LIKE 模式，并转义 LIKE 元字符。
fn like_pattern(token: &str) -> String {
    let mut pattern = String::with_capacity(token.len() + 2);
    pattern.push('%');
    for ch in token.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

/// 关键词搜索。每个词必须命中名称、描述或分类名之一（词间 AND、字段间 OR），
/// 结果按名称排序。空查询返回空列表。
pub fn search_keyword(conn: &Connection, query: &str) -> FinditResult<Vec<KeywordHit>> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    let mut clauses: Vec<String> = Vec::with_capacity(tokens.len());
    let mut binds: Vec<String> = Vec::with_capacity(tokens.len() * 3);
    for token in &tokens {
        let pattern = like_pattern(&token.to_lowercase());
        clauses.push(
            "(lower(i.name) LIKE ? ESCAPE '\\' \
             OR lower(i.description) LIKE ? ESCAPE '\\' \
             OR EXISTS (SELECT 1 FROM item_categories ic \
                        JOIN categories c ON c.id = ic.category_id \
                        WHERE ic.item_id = i.id \
                        AND lower(c.name) LIKE ? ESCAPE '\\'))"
                .to_string(),
        );
        binds.extend([pattern.clone(), pattern.clone(), pattern]);
    }

    let sql = format!(
        "SELECT i.id, i.name, i.description, i.quantity, i.photo_path, i.box_id, \
                COALESCE(i.created_at, ''), COALESCE(i.updated_at, ''), \
                b.name, u.name \
         FROM items i \
         JOIN storage_boxes b ON b.id = i.box_id \
         JOIN storage_units u ON u.id = b.unit_id \
         WHERE {} \
         ORDER BY i.name",
        clauses.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(i64, String, String, i64, Option<String>, i64, String, String, String, String)> =
        stmt.query_map(params_from_iter(binds), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let cats = load_item_categories(conn, &ids)?;

    Ok(rows
        .into_iter()
        .map(|(id, name, description, quantity, photo_path, box_id, created_at, updated_at, box_name, unit_name)| {
            let mut categories = cats.get(&id).cloned().unwrap_or_default();
            categories.sort();
            KeywordHit {
                item: Item {
                    id,
                    name,
                    description,
                    quantity,
                    photo_path,
                    box_id,
                    categories,
                    created_at,
                    updated_at,
                },
                box_name,
                unit_name,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::{boxes, categories, items, units};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn fixture(conn: &Connection) {
        let unit = units::create_unit(conn, "车库", "门口的铁柜").unwrap();
        let box_ = boxes::create_box(conn, unit.id, "工具箱", "红色塑料箱").unwrap();
        let tools = categories::create_category(conn, "五金").unwrap();
        items::create_item(conn, box_.id, "博世电钻套装", "含 20 个钻头", 1, &[tools.id]).unwrap();
        items::create_item(conn, box_.id, "蓝色收纳箱", "装换季衣物", 1, &[]).unwrap();
        items::create_item(conn, box_.id, "Power Drill", "cordless drill", 1, &[]).unwrap();
        items::create_item(conn, box_.id, "水管扳手", "", 2, &[tools.id]).unwrap();
        items::create_item(conn, box_.id, "螺丝刀", "修水管的工具", 1, &[]).unwrap();
    }

    fn hit_names(hits: &[KeywordHit]) -> Vec<&str> {
        hits.iter().map(|h| h.item.name.as_str()).collect()
    }

    #[test]
    fn tokenize_splits_whitespace() {
        assert_eq!(tokenize("  蓝色\t箱子 "), vec!["蓝色".to_string(), "箱子".to_string()]);
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn chinese_substring_match() {
        let conn = setup();
        fixture(&conn);
        let hits = search_keyword(&conn, "电钻").unwrap();
        assert_eq!(hit_names(&hits), vec!["博世电钻套装"]);
    }

    #[test]
    fn multi_token_and() {
        let conn = setup();
        fixture(&conn);
        // 两个词同时命中「蓝色收纳箱」
        let hits = search_keyword(&conn, "蓝色 箱").unwrap();
        assert_eq!(hit_names(&hits), vec!["蓝色收纳箱"]);
        // 任一不满足则不命中
        assert!(search_keyword(&conn, "蓝色 扳手").unwrap().is_empty());
    }

    #[test]
    fn description_field_or() {
        let conn = setup();
        fixture(&conn);
        // 词只出现在 description 中
        let hits = search_keyword(&conn, "修水管").unwrap();
        assert_eq!(hit_names(&hits), vec!["螺丝刀"]);
    }

    #[test]
    fn latin_case_insensitive() {
        let conn = setup();
        fixture(&conn);
        let hits = search_keyword(&conn, "drill").unwrap();
        assert_eq!(hit_names(&hits), vec!["Power Drill"]);
    }

    #[test]
    fn category_name_match() {
        let conn = setup();
        fixture(&conn);
        let hits = search_keyword(&conn, "五金").unwrap();
        assert_eq!(hit_names(&hits), vec!["博世电钻套装", "水管扳手"]);
    }

    #[test]
    fn empty_query_returns_empty() {
        let conn = setup();
        fixture(&conn);
        assert!(search_keyword(&conn, "").unwrap().is_empty());
        assert!(search_keyword(&conn, "   ").unwrap().is_empty());
    }

    #[test]
    fn hit_includes_box_and_unit_names() {
        let conn = setup();
        fixture(&conn);
        let hits = search_keyword(&conn, "电钻").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].box_name, "工具箱");
        assert_eq!(hits[0].unit_name, "车库");
        assert_eq!(hits[0].item.categories, vec!["五金".to_string()]);
        assert!(hits[0].item.description.contains("钻头"));
    }

    #[test]
    fn like_metacharacters_are_escaped() {
        let conn = setup();
        units::create_unit(&conn, "柜子", "").unwrap();
        let unit_id: i64 = conn
            .query_row("SELECT id FROM storage_units", [], |r| r.get(0))
            .unwrap();
        let box_ = boxes::create_box(&conn, unit_id, "箱子", "").unwrap();
        items::create_item(&conn, box_.id, "100%棉布", "", 1, &[]).unwrap();
        // `%` 应作为字面量而非通配符
        assert_eq!(hit_names(&search_keyword(&conn, "100%").unwrap()), vec!["100%棉布"]);
        assert!(search_keyword(&conn, "100_").unwrap().is_empty());
    }
}
