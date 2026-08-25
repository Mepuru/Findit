//! 数据访问层：纯函数，接收 `&rusqlite::Connection`，零全局状态，
//! 便于用内存库做单元测试。

pub mod boxes;
pub mod categories;
pub mod items;
pub mod settings;
pub mod units;

use rusqlite::Connection;

use crate::core::error::{FinditError, FinditResult};

/// 判断 rusqlite 错误是否为唯一约束冲突。
pub(crate) fn is_unique_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                ..
            },
            _
        )
    )
}

/// 校验名称：去空白后不能为空，最长 200 字符。
pub(crate) fn validate_name(entity: &str, raw: &str) -> FinditResult<String> {
    let name = raw.trim().to_string();
    if name.is_empty() {
        return Err(FinditError::Validation(format!("{entity}名称不能为空")));
    }
    if name.chars().count() > 200 {
        return Err(FinditError::Validation(format!(
            "{entity}名称过长（最多 200 个字符）"
        )));
    }
    Ok(name)
}

/// 批量读取若干物品的分类名列表，返回 item_id → 分类名 Vec。
pub(crate) fn load_item_categories(
    conn: &Connection,
    item_ids: &[i64],
) -> FinditResult<std::collections::HashMap<i64, Vec<String>>> {
    let mut result: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    if item_ids.is_empty() {
        return Ok(result);
    }
    let placeholders = std::iter::repeat_n("?", item_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT ic.item_id, c.name FROM item_categories ic \
         JOIN categories c ON c.id = ic.category_id \
         WHERE ic.item_id IN ({placeholders}) \
         ORDER BY c.name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(item_ids.iter().copied()), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (item_id, name) = row?;
        result.entry(item_id).or_default().push(name);
    }
    Ok(result)
}
