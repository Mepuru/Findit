//! 关键词搜索：空白分词，词间 AND、字段间（名称/描述/分类名）OR。
//!
//! 拉丁词大小写不敏感：SQL 双侧使用 `lower()` 折叠（SQLite 原生
//! `lower()` 仅覆盖 ASCII）；查询词在 Rust 侧也只做 ASCII 折叠
//! （[`fold_ascii`]），保证两侧语义一致（重音等非 ASCII 字符不折叠）。
//! 中文无大小写问题，直接按子串匹配。
//!
//! ## P-M1：FTS5 索引替代 LIKE 全表扫描
//!
//! 名称/描述检索从 `LIKE '%x%'` 全表扫描升级为 FTS5 虚拟表
//! `items_fts`（`tokenize='unicode61'`）。bundled SQLite（3.50.x，
//! 见 libsqlite3-sys）未编译 ngram tokenizer（amalgamation 中无 ngram
//! 代码），而 unicode61 把「连续 CJK 串」当作单个 token，无法做中文
//! 子串匹配。因此索引与查询两侧统一使用 [`cjk_spaced`]：把 CJK 字符
//! 逐个以空格分隔，每字成为独立 token，FTS5 短语查询即还原子串语义：
//!
//! - **大小写语义**：FTS 表内存储 `lower()` 折叠后的文本（SQLite 原生
//!   `lower()` 只折叠 ASCII），查询词做同样的 ASCII 折叠；
//! - **中文子串/单字**：`电钻` → 短语 (电,钻) 相邻 token，命中
//!   「博世电钻套装」；`箱` 单字即 token，无需退化 LIKE；
//! - **与 SQLite `lower()` 语义不一致的场景回退 LIKE**（见 [`needs_like`]）：
//!   ① 含 LIKE 元字符（`%` `_` `\`）的词须保持字面语义；
//!   ② 含非 ASCII 大写字母的词——unicode61 会做 Unicode 大小写折叠与去音调
//!   （`CAFÉ`→`cafe`），与「仅 ASCII 折叠」的历史语义不符；
//! - **分类名检索**：仍走 `item_categories JOIN categories` 的 LIKE
//!   （分类表远小于物品表）。
//!
//! 索引同步与写入路径解耦：`items_fts` 由 `items` 表上的触发器维护
//! （INSERT / UPDATE / DELETE），任何写入路径（含 repo 层、恢复库）
//! 都自动同步。[`ensure_fts_schema`] 幂等，建表后回填存量数据，
//! 在 `core::db::init_db` 与首次搜索时都会执行。
//! bundled SQLite 已启用 `SQLITE_ENABLE_FTS5`（libsqlite3-sys 编译期验证）。

use std::collections::HashSet;

use rusqlite::{params, params_from_iter, Connection};

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

/// 仅折叠 ASCII 大写字母，与 SQLite 原生 `lower()` 行为一致。
/// 非 ASCII 字符（如重音字母、中文）原样保留。
fn fold_ascii(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii() { c.to_ascii_lowercase() } else { c })
        .collect()
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

/// FTS5 虚拟表名（名称/描述子串索引，unicode61 tokenizer）。
const FTS_TABLE: &str = "items_fts";

/// 幂等创建 FTS5 索引与同步触发器，并回填存量数据。
///
/// - `CREATE VIRTUAL TABLE IF NOT EXISTS` / `CREATE TRIGGER IF NOT EXISTS`；
/// - tokenizer 为内置 `unicode61`（bundled SQLite 未编译 ngram）：unicode61
///   把「连续 CJK 串」当作单个 token、无法子串匹配，故索引与查询两侧均用
///   [`cjk_spaced`] 把 CJK 字符逐字空格分隔（每字一个 token，短语=子串）；
/// - 表为空时把现有全部物品回填进索引（`lower()` 折叠 + CJK 分隔）；
/// - 可在 `init_db`（启动时一次）与每次搜索前（兜底）调用，重复调用开销极小。
pub fn ensure_fts_schema(conn: &Connection) -> FinditResult<()> {
    // 触发器与回填的 SQL 依赖 cjk_spaced 标量函数，先注册（幂等）。
    register_cjk_spaced(conn)?;
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {FTS_TABLE} USING fts5(\n\
             name, description, tokenize = 'unicode61'\n\
         );\n\
         CREATE TRIGGER IF NOT EXISTS trg_items_fts_insert AFTER INSERT ON items BEGIN\n\
           INSERT INTO {FTS_TABLE}(rowid, name, description)\n\
             VALUES (new.id, cjk_spaced(lower(new.name)), cjk_spaced(lower(new.description)));\n\
         END;\n\
         CREATE TRIGGER IF NOT EXISTS trg_items_fts_update AFTER UPDATE OF name, description ON items BEGIN\n\
           UPDATE {FTS_TABLE} SET name = cjk_spaced(lower(new.name)), description = cjk_spaced(lower(new.description))\n\
             WHERE rowid = old.id;\n\
         END;\n\
         CREATE TRIGGER IF NOT EXISTS trg_items_fts_delete AFTER DELETE ON items BEGIN\n\
           DELETE FROM {FTS_TABLE} WHERE rowid = old.id;\n\
         END;"
    ))?;

    // 仅当索引为空时回填存量（触发器已覆盖增量写入）。
    let n: i64 = conn.query_row(
        &format!("SELECT count(*) FROM {FTS_TABLE}"),
        [],
        |row| row.get(0),
    )?;
    if n == 0 {
        conn.execute(
            &format!(
                "INSERT INTO {FTS_TABLE}(rowid, name, description)\n\
                 SELECT id, cjk_spaced(lower(name)), cjk_spaced(lower(description)) FROM items"
            ),
            [],
        )?;
    }
    Ok(())
}

/// 生成 FTS5 短语查询：ASCII 折叠 + CJK 逐字空格分隔 + 双引号转义 + 引号包裹。
///
/// unicode61 tokenizer 把「连续 CJK 串」视为单个 token（无法子串匹配），
/// 因此查询与索引两侧都先经 [`cjk_spaced`] 把 CJK 字符逐个空格分隔，
/// 每字成为独立 token，短语查询即还原「子串包含」语义。
/// 双引号内的 FTS5 元字符（`*` `(` `^` 等）按字面处理。
fn fts_phrase(token: &str) -> String {
    let folded = fold_ascii(token);
    let spaced = cjk_spaced(&folded);
    let escaped = spaced.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

/// CJK 统一表意文字 / 假名 / 谚文范围（含扩展区）。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // 日文假名
        | 0x3400..=0x4DBF // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 统一表意文字
        | 0xAC00..=0xD7AF // 谚文音节
        | 0xF900..=0xFAFF // CJK 兼容表意文字
        | 0x20000..=0x2FA1F) // CJK 扩展 B+
}

/// 在相邻字符之间（至少一方为 CJK）插入空格：
/// `博世电钻` → `博 世 电 钻`；`USB电钻` → `USB 电 钻`；纯 ASCII 原样返回。
/// 索引与查询两侧使用同一变换，保证短语匹配一致。
fn cjk_spaced(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut prev: Option<char> = None;
    for ch in s.chars() {
        let space_needed = match prev {
            Some(p) => {
                !p.is_whitespace() && !ch.is_whitespace() && (is_cjk(p) || is_cjk(ch))
            }
            None => false,
        };
        if space_needed {
            out.push(' ');
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

/// 注册 `cjk_spaced` 标量函数到连接（同名注册即替换，幂等）。
///
/// FTS 触发器的 INSERT/UPDATE 与存量回填都在 SQL 内调用该函数；
/// 任何执行过 [`ensure_fts_schema`] 的连接都会先注册，保证触发器
/// 在后续物品写入时能正常同步索引。
fn register_cjk_spaced(conn: &Connection) -> FinditResult<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "cjk_spaced",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let s: String = ctx.get(0)?;
            Ok(cjk_spaced(&s))
        },
    )?;
    Ok(())
}

/// 该词是否需要走 LIKE（FTS5 unicode61 语义与 SQLite `lower()` 不一致的场景）：
/// - 含 LIKE 元字符（`%` `_` `\`）：unicode61 把它们当分隔符，`100%` 与 `100_`
///   会退化为同一 token `100`，无法保持字面匹配语义；
/// - 含非 ASCII 大写字母：unicode61 做 Unicode 大小写折叠与去音调
///   （`CAFÉ`→`cafe`），而历史语义（SQLite `lower()`）只折叠 ASCII。
fn needs_like(token: &str) -> bool {
    token
        .chars()
        .any(|c| matches!(c, '%' | '_' | '\\') || (c.is_uppercase() && !c.is_ascii()))
}

/// 名称/描述经 FTS5 索引匹配（unicode61：CJK 单字即 token，1 字符词同样可索引）。
fn fts_match_ids(conn: &Connection, token: &str) -> FinditResult<HashSet<i64>> {
    let mut ids: HashSet<i64> = HashSet::new();
    let phrase = fts_phrase(token);
    let mut stmt = conn.prepare(&format!(
        "SELECT rowid FROM {FTS_TABLE} WHERE {FTS_TABLE} MATCH ?1"
    ))?;
    let rows = stmt.query_map(params![phrase], |row| row.get::<_, i64>(0))?;
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// 名称/描述 LIKE 匹配（仅用于 1 字符词等 ngram 无法覆盖的场景）。
fn like_match_ids(conn: &Connection, token: &str) -> FinditResult<HashSet<i64>> {
    let pattern = like_pattern(&fold_ascii(token));
    let mut ids: HashSet<i64> = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT id FROM items \
         WHERE lower(name) LIKE ?1 ESCAPE '\\' OR lower(description) LIKE ?1 ESCAPE '\\'",
    )?;
    let rows = stmt.query_map(params![pattern], |row| row.get::<_, i64>(0))?;
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// 分类名 LIKE 匹配（分类表远小于物品表，保持 LIKE 即可）。
fn category_match_ids(conn: &Connection, token: &str) -> FinditResult<HashSet<i64>> {
    let pattern = like_pattern(&fold_ascii(token));
    let mut ids: HashSet<i64> = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT ic.item_id FROM item_categories ic \
         JOIN categories c ON c.id = ic.category_id \
         WHERE lower(c.name) LIKE ?1 ESCAPE '\\'",
    )?;
    let rows = stmt.query_map(params![pattern], |row| row.get::<_, i64>(0))?;
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// 关键词搜索。每个词必须命中名称、描述或分类名之一（词间 AND、字段间 OR），
/// 结果按名称排序。空查询返回空列表。
///
/// 实现：按词取「名称/描述（FTS5 为主，LIKE 兜底见 [`needs_like`]）
/// ∪ 分类名（LIKE）」的 id 集合，再对所有词取交集。
pub fn search_keyword(conn: &Connection, query: &str) -> FinditResult<Vec<KeywordHit>> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    // 兜底确保 FTS 索引存在（启动时 init_db 已建，这里防御性幂等执行）。
    ensure_fts_schema(conn)?;

    let mut final_ids: Option<HashSet<i64>> = None;
    for token in &tokens {
        // unicode61 与 SQLite lower() 语义不一致的词（LIKE 元字符、
        // 非 ASCII 大写）走 LIKE，其余走 FTS5 短语匹配。
        let mut ids = if needs_like(token) {
            like_match_ids(conn, token)?
        } else {
            fts_match_ids(conn, token)?
        };
        ids.extend(category_match_ids(conn, token)?);
        final_ids = Some(match final_ids {
            None => ids,
            Some(prev) => prev.intersection(&ids).copied().collect(),
        });
        if final_ids.as_ref().is_some_and(|s| s.is_empty()) {
            break;
        }
    }

    let ids: Vec<i64> = final_ids.unwrap_or_default().into_iter().collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    load_hits(conn, &ids)
}

/// 按 id 批量加载关键词命中（物品 + 所在箱/单元名），按名称排序。
///
/// `IN` 按 900 一批分块，兼容旧版 SQLite 的变量数上限（999）。
fn load_hits(conn: &Connection, ids: &[i64]) -> FinditResult<Vec<KeywordHit>> {
    const CHUNK: usize = 900;
    let mut all_rows: Vec<(i64, String, String, i64, Option<String>, i64, String, String, String, String)> =
        Vec::new();
    for chunk in ids.chunks(CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT i.id, i.name, i.description, i.quantity, i.photo_path, i.box_id, \
                    COALESCE(i.created_at, ''), COALESCE(i.updated_at, ''), \
                    b.name, u.name \
             FROM items i \
             JOIN storage_boxes b ON b.id = i.box_id \
             JOIN storage_units u ON u.id = b.unit_id \
             WHERE i.id IN ({placeholders}) \
             ORDER BY i.name"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter().copied()), |row| {
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
        })?;
        for row in rows {
            all_rows.push(row?);
        }
    }
    // 分块加载破坏了全局排序：这里按名称整体排序一次。
    all_rows.sort_by(|a, b| a.1.cmp(&b.1));

    let chunk_ids: Vec<i64> = all_rows.iter().map(|r| r.0).collect();
    let cats = load_item_categories(conn, &chunk_ids)?;

    Ok(all_rows
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
    fn cjk_spacing_separates_each_cjk_char() {
        // 纯 CJK：逐字分隔
        assert_eq!(cjk_spaced("博世电钻"), "博 世 电 钻");
        // 与 ASCII 混合：仅在 CJK 邻接处插入空格
        assert_eq!(cjk_spaced("USB电钻"), "USB 电 钻");
        assert_eq!(cjk_spaced("电钻USB"), "电 钻 USB");
        // 纯 ASCII / 已有空格：原样保留
        assert_eq!(cjk_spaced("Power Drill"), "Power Drill");
        assert_eq!(cjk_spaced("电 钻"), "电 钻");
        // 大小写折叠不影响分隔
        assert_eq!(cjk_spaced("Café 咖啡"), "Café 咖 啡");
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
    fn non_ascii_chars_fold_like_sqlite_lower() {
        // SQLite 原生 lower() 只折叠 ASCII；Rust 侧必须保持一致，
        // 否则会出现「查询词被折叠而列值未被折叠」的错位。
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let box_ = boxes::create_box(&conn, unit.id, "箱子", "").unwrap();
        items::create_item(&conn, box_.id, "Café Grinder", "手摇磨豆机", 1, &[]).unwrap();

        // 重音字符大小写一致 + ASCII 大小写不敏感 → 命中。
        let hits = search_keyword(&conn, "café GRINDER").unwrap();
        assert_eq!(hit_names(&hits), vec!["Café Grinder"]);

        // 重音字符大小写不一致时不折叠（与 SQLite 行为一致，不命中）。
        assert!(search_keyword(&conn, "CAFÉ grinder").unwrap().is_empty());
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

    #[test]
    fn fts_schema_is_idempotent() {
        let conn = setup();
        ensure_fts_schema(&conn).unwrap();
        ensure_fts_schema(&conn).unwrap(); // 重复调用不报错
        let n: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE name='items_fts'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn fts_triggers_sync_writes() {
        // P-M1 回归：索引由触发器维护，任何写入（含 repo 层）自动同步。
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let box_ = boxes::create_box(&conn, unit.id, "箱子", "").unwrap();
        let item = items::create_item(&conn, box_.id, "旧名字", "", 1, &[]).unwrap();

        // 建索引后写入的物品也能被搜到（回填只针对存量）。
        ensure_fts_schema(&conn).unwrap();
        assert_eq!(hit_names(&search_keyword(&conn, "旧名字").unwrap()), vec!["旧名字"]);

        // UPDATE 同步
        items::update_item(
            &conn,
            item.id,
            Some("新名字".to_string()),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(search_keyword(&conn, "旧名字").unwrap().is_empty());
        assert_eq!(hit_names(&search_keyword(&conn, "新名字").unwrap()), vec!["新名字"]);

        // DELETE 同步
        items::delete_item(&conn, item.id).unwrap();
        assert!(search_keyword(&conn, "新名字").unwrap().is_empty());
    }

    #[test]
    fn single_char_token_matches_via_fts() {
        // unicode61 把每个 CJK 字符视为独立 token：单字查询可直接走 FTS5。
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let box_ = boxes::create_box(&conn, unit.id, "箱子", "").unwrap();
        items::create_item(&conn, box_.id, "箱中宝物", "", 1, &[]).unwrap();
        let hits = search_keyword(&conn, "箱").unwrap();
        assert_eq!(hit_names(&hits), vec!["箱中宝物"]);
    }

    #[test]
    fn fts_backfill_covers_pre_existing_items() {
        // 存量数据（建索引前已存在）也应被回填进 FTS 索引。
        let conn = setup();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let box_ = boxes::create_box(&conn, unit.id, "箱子", "").unwrap();
        items::create_item(&conn, box_.id, "存量物品", "", 1, &[]).unwrap();
        let hits = search_keyword(&conn, "存量").unwrap();
        assert_eq!(hit_names(&hits), vec!["存量物品"]);
    }
}
