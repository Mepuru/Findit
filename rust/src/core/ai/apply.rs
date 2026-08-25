//! 意图应用：把确认后的 [`ParsedIntent`] 写入数据库。
//!
//! - 建档（create）：单元/收纳箱不存在时按名称自动创建；
//!   同名实体优先复用（关联而非重复创建）；
//! - 修改（modify）：按 `target_query` 关键词搜索定位目标物品（取第一个命中），
//!   支持移动（可顺带建箱建单元）、改名、改备注、改数量；
//! - 全过程在单个事务内执行，任何一步失败整体回滚。

use rusqlite::{params, Connection};

use crate::api::model::Item;
use crate::core::ai::parse::ParsedIntent;
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo::{boxes, items, units};
use crate::core::search::keyword;

/// 未指定存储单元且无法推导时的默认单元名。
pub const DEFAULT_UNIT_NAME: &str = "默认单元";

/// 实体引用：id + 名称 + 是否本次新建。
#[derive(Debug, Clone, PartialEq)]
pub struct EntityRef {
    pub id: i64,
    pub name: String,
    pub created: bool,
}

/// 建档结果：单元、收纳箱（含新建/复用标记）与入库物品。
#[derive(Debug, Clone, PartialEq)]
pub struct QuickAddResult {
    pub unit: EntityRef,
    pub storage_box: EntityRef,
    pub item: Item,
}

/// 修改结果：更新后的物品、变更到的收纳箱（无移动为 `None`）与已应用变更描述。
#[derive(Debug, Clone, PartialEq)]
pub struct ModifyResult {
    pub item: Item,
    pub moved_box: Option<EntityRef>,
    pub changes: Vec<String>,
}

// ---------------------------------------------------------------------------
// 建档
// ---------------------------------------------------------------------------

/// 应用建档意图（单事务）。
pub fn apply_create(conn: &Connection, intent: &ParsedIntent) -> FinditResult<QuickAddResult> {
    let item_name = required(intent.item_name.as_deref(), "物品名称")?;
    let box_name = required(intent.box_name.as_deref(), "目标收纳箱名称")?;
    let description = intent.item_description.clone().unwrap_or_default();
    let quantity = intent.quantity.unwrap_or(1);
    if quantity < 1 {
        return Err(FinditError::Validation("物品数量必须大于等于 1".to_string()));
    }

    let tx = conn.unchecked_transaction()?;

    // 用户指定的单元：按名称查找或创建。
    let named_unit: Option<EntityRef> = match &intent.unit_name {
        Some(name) => Some(find_or_create_unit(&tx, name)?),
        None => None,
    };

    // 收纳箱：同名优先复用（优先用户指定单元下的同名箱）。
    let candidates = boxes_with_name(&tx, &box_name)?;
    let preferred_unit_id = named_unit.as_ref().map(|u| u.id);
    let picked = candidates
        .iter()
        .find(|(_, unit_id)| Some(*unit_id) == preferred_unit_id)
        .or_else(|| candidates.first());

    let (unit_ref, box_ref) = match picked {
        Some((box_id, unit_id)) => (
            unit_ref_by_id(&tx, *unit_id)?,
            EntityRef {
                id: *box_id,
                name: box_name.clone(),
                created: false,
            },
        ),
        None => {
            let unit_ref = match named_unit {
                Some(u) => u,
                None => find_or_create_unit(&tx, DEFAULT_UNIT_NAME)?,
            };
            let box_ = boxes::create_box(&tx, unit_ref.id, &box_name, "")?;
            (
                unit_ref,
                EntityRef {
                    id: box_.id,
                    name: box_.name,
                    created: true,
                },
            )
        }
    };

    let item = items::create_item(&tx, box_ref.id, &item_name, &description, quantity, &[])?;
    tx.commit()?;

    Ok(QuickAddResult {
        unit: unit_ref,
        storage_box: box_ref,
        item,
    })
}

// ---------------------------------------------------------------------------
// 修改
// ---------------------------------------------------------------------------

/// 应用修改意图（单事务）。目标物品为 `target_query` 关键词搜索的第一个命中。
pub fn apply_modify(conn: &Connection, intent: &ParsedIntent) -> FinditResult<ModifyResult> {
    let query = required(intent.target_query.as_deref(), "目标物品描述")?;

    let has_box_change = intent.new_box_name.is_some() || intent.new_unit_name.is_some();
    let has_field_change = intent.new_item_name.is_some()
        || intent.new_description.is_some()
        || intent.new_quantity.is_some();
    if !has_box_change && !has_field_change {
        return Err(FinditError::Validation(
            "没有有效的变更字段，请至少指定新箱/新单元/新名称/新数量/新备注之一".to_string(),
        ));
    }

    let tx = conn.unchecked_transaction()?;

    let hits = keyword::search_keyword(&tx, &query)?;
    let target = hits.into_iter().next().ok_or_else(|| FinditError::NotFound {
        entity: "物品".to_string(),
        hint: format!("没有匹配「{query}」的物品"),
    })?;

    let mut changes: Vec<String> = Vec::new();
    let mut box_out: Option<EntityRef> = None;

    // 移动：解析目标箱（不存在则按名称创建），再更新物品的箱归属。
    if let Some(raw_box_name) = &intent.new_box_name {
        let box_name = required(Some(raw_box_name.as_str()), "新收纳箱名称")?;
        // 目标单元：显式指定（不存在则创建）或沿用物品当前所在单元。
        let unit_id = match &intent.new_unit_name {
            Some(unit_name) => find_or_create_unit(&tx, unit_name)?.id,
            None => unit_of_box(&tx, target.item.box_id)?,
        };
        let box_ref = ensure_box(&tx, unit_id, &box_name)?;
        if box_ref.id != target.item.box_id {
            items::update_item(&tx, target.item.id, None, None, None, Some(box_ref.id), None)?;
            changes.push(format!("移动到「{}」", box_ref.name));
        }
        box_out = Some(box_ref);
    } else if let Some(unit_name) = &intent.new_unit_name {
        // 仅换单元：把物品移入目标单元下与其当前所在箱同名的箱，
        // 找不到时在目标单元下创建同名箱。
        let unit_id = find_or_create_unit(&tx, unit_name)?.id;
        let current_box_name: String = tx.query_row(
            "SELECT name FROM storage_boxes WHERE id = ?1",
            params![target.item.box_id],
            |row| row.get(0),
        )?;
        let box_ref = ensure_box_in_unit(&tx, unit_id, &current_box_name)?;
        if box_ref.id != target.item.box_id {
            items::update_item(&tx, target.item.id, None, None, None, Some(box_ref.id), None)?;
            changes.push(format!("移动到「{}」", box_ref.name));
        }
        box_out = Some(box_ref);
    }

    // 字段变更：名称 / 备注 / 数量。
    let new_name = intent.new_item_name.clone();
    let new_description = intent.new_description.clone();
    let new_quantity = intent.new_quantity;
    if new_name.is_some() || new_description.is_some() || new_quantity.is_some() {
        if let Some(q) = new_quantity {
            if q < 1 {
                return Err(FinditError::Validation("物品数量必须大于等于 1".to_string()));
            }
        }
        items::update_item(
            &tx,
            target.item.id,
            new_name.clone(),
            new_description.clone(),
            new_quantity,
            None,
            None,
        )?;
        if let Some(name) = &new_name {
            changes.push(format!("改名为「{name}」"));
        }
        if let Some(desc) = &new_description {
            changes.push(if desc.is_empty() {
                "清空备注".to_string()
            } else {
                format!("备注改为「{desc}」")
            });
        }
        if let Some(q) = new_quantity {
            changes.push(format!("数量改为 {q}"));
        }
    }

    let item = items::get_item(&tx, target.item.id)?;
    tx.commit()?;

    Ok(ModifyResult {
        item,
        moved_box: box_out,
        changes,
    })
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

fn required(raw: Option<&str>, field: &str) -> FinditResult<String> {
    let value = raw.map(str::trim).unwrap_or_default().to_string();
    if value.is_empty() {
        return Err(FinditError::Validation(format!(
            "{field}不能为空，请在预览卡片中补充"
        )));
    }
    Ok(value)
}

/// 按名称查找单元，不存在则创建。
fn find_or_create_unit(conn: &Connection, name: &str) -> FinditResult<EntityRef> {
    match units::create_unit(conn, name, "") {
        Ok(unit) => Ok(EntityRef {
            id: unit.id,
            name: unit.name,
            created: true,
        }),
        Err(FinditError::DuplicateName { .. }) => {
            let name = name.trim().to_string();
            let id: i64 = conn.query_row(
                "SELECT id FROM storage_units WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )?;
            Ok(EntityRef {
                id,
                name,
                created: false,
            })
        }
        Err(e) => Err(e),
    }
}

/// 同名收纳箱列表（按 id 升序），返回 (box_id, unit_id)。
fn boxes_with_name(conn: &Connection, name: &str) -> FinditResult<Vec<(i64, i64)>> {
    let mut stmt =
        conn.prepare("SELECT id, unit_id FROM storage_boxes WHERE name = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![name.trim()], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// 读取单元的引用信息。
fn unit_ref_by_id(conn: &Connection, unit_id: i64) -> FinditResult<EntityRef> {
    let name: String = conn.query_row(
        "SELECT name FROM storage_units WHERE id = ?1",
        params![unit_id],
        |row| row.get(0),
    )?;
    Ok(EntityRef {
        id: unit_id,
        name,
        created: false,
    })
}

/// 收纳箱所在单元 id。
fn unit_of_box(conn: &Connection, box_id: i64) -> FinditResult<i64> {
    match conn.query_row(
        "SELECT unit_id FROM storage_boxes WHERE id = ?1",
        params![box_id],
        |row| row.get(0),
    ) {
        Ok(id) => Ok(id),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(FinditError::NotFound {
            entity: "收纳箱".to_string(),
            hint: format!("id={box_id}"),
        }),
        Err(e) => Err(e.into()),
    }
}

/// 只在目标单元下找同名箱；找不到时在该单元下新建（不回退其它单元的同名箱）。
fn ensure_box_in_unit(conn: &Connection, unit_id: i64, name: &str) -> FinditResult<EntityRef> {
    if let Some((box_id, _)) = boxes_with_name(conn, name)?
        .into_iter()
        .find(|(_, uid)| *uid == unit_id)
    {
        return Ok(EntityRef {
            id: box_id,
            name: name.trim().to_string(),
            created: false,
        });
    }
    let box_ = boxes::create_box(conn, unit_id, name, "")?;
    Ok(EntityRef {
        id: box_.id,
        name: box_.name,
        created: true,
    })
}

/// 按名称找箱：优先目标单元下的同名箱 → 任意同名箱 → 在目标单元新建。
fn ensure_box(conn: &Connection, unit_id: i64, name: &str) -> FinditResult<EntityRef> {
    let candidates = boxes_with_name(conn, name)?;
    if let Some((box_id, _)) = candidates.iter().find(|(_, uid)| *uid == unit_id) {
        return Ok(EntityRef {
            id: *box_id,
            name: name.trim().to_string(),
            created: false,
        });
    }
    if let Some((box_id, _)) = candidates.first() {
        return Ok(EntityRef {
            id: *box_id,
            name: name.trim().to_string(),
            created: false,
        });
    }
    let box_ = boxes::create_box(conn, unit_id, name, "")?;
    Ok(EntityRef {
        id: box_.id,
        name: box_.name,
        created: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ai::parse::IntentKind;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::categories;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn create_intent() -> ParsedIntent {
        let mut intent = ParsedIntent::new_create();
        intent.unit_name = Some("车库".to_string());
        intent.box_name = Some("蓝色箱子".to_string());
        intent.item_name = Some("电钻".to_string());
        intent
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    // ---------- 建档 ----------

    #[test]
    fn create_auto_creates_unit_and_box() {
        let conn = setup();
        let result = apply_create(&conn, &create_intent()).unwrap();
        assert!(result.unit.created);
        assert_eq!(result.unit.name, "车库");
        assert!(result.storage_box.created);
        assert_eq!(result.storage_box.name, "蓝色箱子");
        assert_eq!(result.item.name, "电钻");
        assert_eq!(result.item.quantity, 1);
        assert_eq!(result.item.box_id, result.storage_box.id);
    }

    #[test]
    fn create_reuses_existing_unit_and_box_by_name() {
        let conn = setup();
        let first = apply_create(&conn, &create_intent()).unwrap();

        // 同名再来一次：单元/箱复用，只新增物品。
        let mut again = create_intent();
        again.item_name = Some("角磨机".to_string());
        let second = apply_create(&conn, &again).unwrap();

        assert!(!second.unit.created);
        assert!(!second.storage_box.created);
        assert_eq!(second.unit.id, first.unit.id);
        assert_eq!(second.storage_box.id, first.storage_box.id);
        assert_eq!(count(&conn, "storage_units"), 1);
        assert_eq!(count(&conn, "storage_boxes"), 1);
        assert_eq!(count(&conn, "items"), 2);
    }

    #[test]
    fn create_without_unit_infers_from_existing_box() {
        let conn = setup();
        apply_create(&conn, &create_intent()).unwrap();

        // 不指定单元，但指定了已存在的箱 → 复用该箱及其单元。
        let mut intent = ParsedIntent::new_create();
        intent.box_name = Some("蓝色箱子".to_string());
        intent.item_name = Some("锤子".to_string());
        let result = apply_create(&conn, &intent).unwrap();
        assert!(!result.unit.created);
        assert_eq!(result.unit.name, "车库");
        assert!(!result.storage_box.created);
        assert_eq!(count(&conn, "storage_units"), 1);
    }

    #[test]
    fn create_without_unit_and_unknown_box_uses_default_unit() {
        let conn = setup();
        let mut intent = ParsedIntent::new_create();
        intent.box_name = Some("神秘箱".to_string());
        intent.item_name = Some("手电".to_string());
        let result = apply_create(&conn, &intent).unwrap();
        assert!(result.unit.created);
        assert_eq!(result.unit.name, DEFAULT_UNIT_NAME);
        assert!(result.storage_box.created);
    }

    #[test]
    fn create_prefers_same_name_box_in_named_unit() {
        let conn = setup();
        // 两个单元下各建一个「工具箱」。
        let u1 = units::create_unit(&conn, "车库", "").unwrap();
        let u2 = units::create_unit(&conn, "阳台", "").unwrap();
        boxes::create_box(&conn, u1.id, "工具箱", "").unwrap();
        let b2 = boxes::create_box(&conn, u2.id, "工具箱", "").unwrap();

        // 指定单元「阳台」→ 应选中阳台下的工具箱。
        let mut intent = ParsedIntent::new_create();
        intent.unit_name = Some("阳台".to_string());
        intent.box_name = Some("工具箱".to_string());
        intent.item_name = Some("花盆".to_string());
        let result = apply_create(&conn, &intent).unwrap();
        assert_eq!(result.storage_box.id, b2.id);
        assert!(!result.storage_box.created);
        assert_eq!(result.unit.name, "阳台");
    }

    #[test]
    fn create_validation_and_transaction_rollback() {
        let conn = setup();

        // 缺物品名
        let mut intent = create_intent();
        intent.item_name = None;
        assert!(matches!(
            apply_create(&conn, &intent),
            Err(FinditError::Validation(_))
        ));

        // 缺箱名
        let mut intent = create_intent();
        intent.box_name = None;
        assert!(matches!(
            apply_create(&conn, &intent),
            Err(FinditError::Validation(_))
        ));

        // 数量非法 → 事务回滚：前面查找/创建的单元与箱不落库。
        let mut intent = create_intent();
        intent.quantity = Some(0);
        assert!(matches!(
            apply_create(&conn, &intent),
            Err(FinditError::Validation(_))
        ));
        assert_eq!(count(&conn, "storage_units"), 0);
        assert_eq!(count(&conn, "storage_boxes"), 0);
        assert_eq!(count(&conn, "items"), 0);
    }

    #[test]
    fn create_with_description_and_quantity() {
        let conn = setup();
        let mut intent = create_intent();
        intent.item_description = Some("含钻头套装".to_string());
        intent.quantity = Some(3);
        let result = apply_create(&conn, &intent).unwrap();
        assert_eq!(result.item.description, "含钻头套装");
        assert_eq!(result.item.quantity, 3);
    }

    // ---------- 修改 ----------

    fn fixture_for_modify(conn: &Connection) -> (i64, i64) {
        let unit = units::create_unit(conn, "车库", "").unwrap();
        let box_ = boxes::create_box(conn, unit.id, "工具箱", "").unwrap();
        let item = items::create_item(conn, box_.id, "扳手", "修水管的工具", 1, &[]).unwrap();
        (unit.id, item.id)
    }

    #[test]
    fn modify_moves_to_existing_box() {
        let conn = setup();
        let (unit_id, item_id) = fixture_for_modify(&conn);
        let other = boxes::create_box(&conn, unit_id, "杂物箱", "").unwrap();

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        intent.new_box_name = Some("杂物箱".to_string());
        let result = apply_modify(&conn, &intent).unwrap();

        assert_eq!(result.item.id, item_id);
        assert_eq!(result.item.box_id, other.id);
        assert!(result.moved_box.is_some());
        assert!(!result.moved_box.as_ref().unwrap().created);
        assert_eq!(result.changes.len(), 1);
        assert!(result.changes[0].contains("杂物箱"));
        assert_eq!(count(&conn, "storage_boxes"), 2); // 没有新建箱
    }

    #[test]
    fn modify_creates_missing_box_in_current_unit() {
        let conn = setup();
        let (unit_id, _item_id) = fixture_for_modify(&conn);

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        intent.new_box_name = Some("红色箱子".to_string());
        let result = apply_modify(&conn, &intent).unwrap();

        let box_ref = result.moved_box.unwrap();
        assert!(box_ref.created);
        assert_eq!(result.item.box_id, box_ref.id);
        // 新箱建在物品当前所在单元（车库）
        let uid: i64 = conn
            .query_row(
                "SELECT unit_id FROM storage_boxes WHERE id = ?1",
                params![box_ref.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uid, unit_id);
    }

    #[test]
    fn modify_with_new_unit_creates_unit_and_box() {
        let conn = setup();
        let (_unit_id, item_id) = fixture_for_modify(&conn);

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        intent.new_unit_name = Some("厨房".to_string());
        intent.new_box_name = Some("水槽下柜子".to_string());
        let result = apply_modify(&conn, &intent).unwrap();

        assert_eq!(result.item.id, item_id);
        assert_eq!(count(&conn, "storage_units"), 2);
        let uid: i64 = conn
            .query_row(
                "SELECT unit_id FROM storage_boxes WHERE id = ?1",
                params![result.item.box_id],
                |r| r.get(0),
            )
            .unwrap();
        let uname: String = conn
            .query_row(
                "SELECT name FROM storage_units WHERE id = ?1",
                params![uid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uname, "厨房");
    }

    #[test]
    fn modify_renames_and_sets_quantity_and_description() {
        let conn = setup();
        let (_unit_id, item_id) = fixture_for_modify(&conn);

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        intent.new_item_name = Some("大号扳手".to_string());
        intent.new_quantity = Some(2);
        intent.new_description = Some("工业级".to_string());
        let result = apply_modify(&conn, &intent).unwrap();

        assert_eq!(result.item.id, item_id);
        assert_eq!(result.item.name, "大号扳手");
        assert_eq!(result.item.quantity, 2);
        assert_eq!(result.item.description, "工业级");
        assert_eq!(result.changes.len(), 3);
        assert!(result.moved_box.is_none());
    }

    #[test]
    fn modify_target_not_found_and_no_changes_errors() {
        let conn = setup();
        fixture_for_modify(&conn);

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("不存在的物品".to_string());
        intent.new_quantity = Some(2);
        assert!(matches!(
            apply_modify(&conn, &intent),
            Err(FinditError::NotFound { .. })
        ));

        // 无变更字段
        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        assert!(matches!(
            apply_modify(&conn, &intent),
            Err(FinditError::Validation(_))
        ));

        // 缺 target_query
        let mut intent = ParsedIntent::new_modify();
        intent.new_quantity = Some(2);
        assert!(matches!(
            apply_modify(&conn, &intent),
            Err(FinditError::Validation(_))
        ));
    }

    #[test]
    fn modify_unit_only_reuses_same_name_box_in_target_unit() {
        let conn = setup();
        let (_unit_id, item_id) = fixture_for_modify(&conn); // 车库/工具箱/扳手
        // 目标单元「厨房」下已有同名箱「工具箱」。
        let kitchen = units::create_unit(&conn, "厨房", "").unwrap();
        let target_box = boxes::create_box(&conn, kitchen.id, "工具箱", "").unwrap();

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        intent.new_unit_name = Some("厨房".to_string());
        let result = apply_modify(&conn, &intent).unwrap();

        assert_eq!(result.item.id, item_id);
        assert_eq!(result.item.box_id, target_box.id);
        assert!(!result.moved_box.as_ref().unwrap().created);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(count(&conn, "storage_boxes"), 2); // 复用而非新建
    }

    #[test]
    fn modify_unit_only_creates_same_name_box_in_target_unit() {
        let conn = setup();
        let (_unit_id, item_id) = fixture_for_modify(&conn);
        let balcony = units::create_unit(&conn, "阳台", "").unwrap();

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        intent.new_unit_name = Some("阳台".to_string());
        let result = apply_modify(&conn, &intent).unwrap();

        let box_ref = result.moved_box.unwrap();
        assert!(box_ref.created);
        assert_eq!(box_ref.name, "工具箱"); // 与物品原所在箱同名
        assert_eq!(result.item.box_id, box_ref.id);
        let uid: i64 = conn
            .query_row(
                "SELECT unit_id FROM storage_boxes WHERE id = ?1",
                params![box_ref.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uid, balcony.id);
        assert_eq!(count(&conn, "storage_units"), 2);
    }

    #[test]
    fn modify_picks_first_matching_item() {
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let box_ = boxes::create_box(&conn, unit.id, "箱子", "").unwrap();
        // 两个都能被「螺丝刀」命中，按名称排序第一个是「一字螺丝刀」。
        items::create_item(&conn, box_.id, "十字螺丝刀", "", 1, &[]).unwrap();
        items::create_item(&conn, box_.id, "一字螺丝刀", "", 1, &[]).unwrap();

        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("螺丝刀".to_string());
        intent.new_quantity = Some(9);
        let result = apply_modify(&conn, &intent).unwrap();
        assert_eq!(result.item.name, "一字螺丝刀");
        assert_eq!(result.item.quantity, 9);
    }

    #[test]
    fn modify_is_transactional_on_failure() {
        let conn = setup();
        let (_unit_id, item_id) = fixture_for_modify(&conn);

        // 数量非法 → 整体回滚（前面的移动不生效）。
        let mut intent = ParsedIntent::new_modify();
        intent.target_query = Some("扳手".to_string());
        intent.new_box_name = Some("新箱".to_string());
        intent.new_quantity = Some(-3);
        assert!(matches!(
            apply_modify(&conn, &intent),
            Err(FinditError::Validation(_))
        ));
        let item = items::get_item(&conn, item_id).unwrap();
        assert_ne!(item.box_id, 0);
        assert_eq!(count(&conn, "storage_boxes"), 1); // 新箱未落库
    }

    #[test]
    fn intent_kinds_are_distinct() {
        assert_ne!(IntentKind::CreateItem, IntentKind::ModifyItem);
        // 分类联查不受影响（覆盖 fixture 中分类路径）
        let conn = setup();
        let cat = categories::create_category(&conn, "五金").unwrap();
        assert!(cat.id > 0);
    }
}
