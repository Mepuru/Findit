//! 一句话意图解析：自然语言 → 结构化意图（建档 / 修改）。
//!
//! 模型输出经过四层容错解析：
//! 1. `serde_json` 直接解析；
//! 2. 剥离 markdown 围栏 / 前后解释文字（取第一段 JSON 对象）；
//! 3. 修复：中英文引号归一、单引号转双引号、去尾逗号、括号重平衡与截断补全；
//! 4. 全部失败返回 [`AiError::ModelOutput`]（附带原始输出摘要，供上层提示重试）。

use serde_json::{Map, Value};

use crate::core::ai::client::AiError;

/// 意图类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    /// 新建物品入档。
    CreateItem,
    /// 修改已有物品（移动、改名、改数量、改备注）。
    ModifyItem,
}

/// 解析出的结构化意图。
///
/// 同时作为「确认应用」的输入：UI 预览时可修正任意字段后原样传回。
/// 建档场景使用 `unit_name/box_name/item_*` 字段；
/// 修改场景使用 `target_query` + `new_*` 字段（`None` 表示不变）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedIntent {
    pub intent: IntentKind,
    /// 建档：目标存储单元名（可空，自动推导或创建默认单元）。
    pub unit_name: Option<String>,
    /// 建档：目标收纳箱名（必填，同名箱会被复用）。
    pub box_name: Option<String>,
    /// 建档：物品名（必填）。
    pub item_name: Option<String>,
    /// 建档：物品备注。
    pub item_description: Option<String>,
    /// 建档：数量（≥1）。
    pub quantity: Option<i64>,
    /// 修改：目标物品的查询描述（按关键词搜索定位）。
    pub target_query: Option<String>,
    /// 修改：移入的存储单元名（可空）。
    pub new_unit_name: Option<String>,
    /// 修改：移入的收纳箱名（同名复用，不存在则创建）。
    pub new_box_name: Option<String>,
    /// 修改：新物品名。
    pub new_item_name: Option<String>,
    /// 修改：新备注。
    pub new_description: Option<String>,
    /// 修改：新数量（≥1）。
    pub new_quantity: Option<i64>,
}

impl ParsedIntent {
    /// 空的建档意图（UI 手动填写用）。
    pub fn new_create() -> Self {
        ParsedIntent {
            intent: IntentKind::CreateItem,
            unit_name: None,
            box_name: None,
            item_name: None,
            item_description: None,
            quantity: None,
            target_query: None,
            new_unit_name: None,
            new_box_name: None,
            new_item_name: None,
            new_description: None,
            new_quantity: None,
        }
    }

    /// 空的修改意图。
    pub fn new_modify() -> Self {
        ParsedIntent {
            intent: IntentKind::ModifyItem,
            ..Self::new_create()
        }
    }
}

/// create_item 的 JSON schema 示例（修复重试提示主用，与 [`PARSE_SYSTEM_PROMPT`] 中一致）。
pub const CREATE_SCHEMA_JSON: &str = r#"{"intent":"create_item","unit_name":"存储单元名","box_name":"收纳箱名","item_name":"物品名","item_description":"备注","quantity":1}"#;

/// modify_item 的 JSON schema 示例（修复重试提示主用，与 [`PARSE_SYSTEM_PROMPT`] 中一致）。
pub const MODIFY_SCHEMA_JSON: &str = r#"{"intent":"modify_item","target_query":"用于定位目标物品的关键词","new_unit_name":"","new_box_name":"","new_item_name":"","new_description":"","new_quantity":0}"#;

/// 意图解析系统提示词。
///
/// 要点：给出两种意图的 JSON schema 示例、要求只输出纯 JSON、
/// 不确定的字段一律留空。
pub const PARSE_SYSTEM_PROMPT: &str = r#"你是家庭收纳应用 Findit 的意图解析助手。把用户的一句话解析为 JSON 对象。

【输出格式】只输出一个纯 JSON 对象：不要解释、不要 markdown 代码块、不要多余文字。

【意图一：新建物品入档】用户想把某件物品收纳/登记到某个地方：
{"intent":"create_item","unit_name":"存储单元名","box_name":"收纳箱名","item_name":"物品名","item_description":"备注","quantity":1}

【意图二：修改已有物品】用户想移动、改名、改数量或改备注某件已有物品：
{"intent":"modify_item","target_query":"用于定位目标物品的关键词","new_unit_name":"","new_box_name":"","new_item_name":"","new_description":"","new_quantity":0}

【规则】
1. 不确定的文本字段一律输出空字符串""，数量不确定时 create_item 给 1、modify_item 给 0。
2. "把X放进/收到/放进Y"且未说明X已登记时，按 create_item 处理。
3. "把X移到/挪到/改名为/数量改成"等针对已有物品的操作，按 modify_item 处理。
4. modify_item 只填用户明确提到的变更字段，其余保持空字符串/0。
5. 名称保持用户原话中的叫法，不要自行扩写。"#;

/// 示例对话（供 UI 提示与测试复用）。
pub const PARSE_EXAMPLES: [&str; 4] = [
    "把电钻放进车库的蓝色箱子",
    "三个充电宝收到书房的抽屉盒里，备注是旧的",
    "把扳手移到厨房的杂物箱",
    "把羽绒服的数量改成2",
];

/// 修复重试提示中各摘要的字符上限（按 `char` 计，UTF-8 安全）。
const REPAIR_SUMMARY_MAX_CHARS: usize = 200;

/// 构造四层容错第 4 层的「修复重试」提示。
///
/// 解析失败后携带原始用户语句、上次模型的错误输出摘要与解析失败原因摘要，
/// 要求模型只输出合法 JSON。`kind` 标识当前场景：建档以 [`CREATE_SCHEMA_JSON`]
/// 为主、修订以 [`MODIFY_SCHEMA_JSON`] 为主（另一 schema 作为备选附上，
/// 意图最终仍由模型按用户原话判断）；过长摘要按字符安全截取（见 [`head`]）。
pub fn build_repair_retry_prompt(
    kind: IntentKind,
    user_text: &str,
    bad_output: &str,
    parse_error: &str,
) -> String {
    let (primary, alternate) = match kind {
        IntentKind::CreateItem => (CREATE_SCHEMA_JSON, MODIFY_SCHEMA_JSON),
        IntentKind::ModifyItem => (MODIFY_SCHEMA_JSON, CREATE_SCHEMA_JSON),
    };
    format!(
        "你上一次的输出未能解析为合法 JSON，请根据下面的错误信息修复并重新输出。\n\
         【你上一次的输出（摘录）】{}\n\
         【解析失败原因】{}\n\
         【用户原话】{}\n\
         【输出要求】只输出一个纯 JSON 对象：不要解释、不要 markdown 代码块、不要多余文字。\n\
         当前场景优先使用以下 schema：\n{}\n\
         若用户意图实际对应另一种操作，则使用：\n{}",
        head(bad_output.trim(), REPAIR_SUMMARY_MAX_CHARS),
        head(parse_error.trim(), REPAIR_SUMMARY_MAX_CHARS),
        head(user_text.trim(), REPAIR_SUMMARY_MAX_CHARS),
        primary,
        alternate,
    )
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// 解析模型原始输出为结构化意图（四层容错）。
pub fn parse_intent_from_output(raw: &str) -> Result<ParsedIntent, AiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AiError::ModelOutput("模型输出为空".to_string()));
    }
    let value = recover_json(trimmed).ok_or_else(|| {
        AiError::ModelOutput(format!(
            "无法从模型输出中解析出 JSON：{}",
            head(trimmed, 200)
        ))
    })?;
    intent_from_value(&value)
}

/// 从 JSON 值映射为 [`ParsedIntent`]。
pub fn intent_from_value(value: &Value) -> Result<ParsedIntent, AiError> {
    let obj = value
        .as_object()
        .ok_or_else(|| AiError::ModelOutput("顶层不是 JSON 对象".to_string()))?;

    let intent_raw = obj
        .get("intent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();

    match intent_raw.as_str() {
        "create_item" | "create" | "add_item" | "add" => parse_create(obj),
        "modify_item" | "modify" | "move_item" | "move" | "update_item" | "update" => {
            parse_modify(obj)
        }
        other => Err(AiError::ModelOutput(format!(
            "无法识别的意图类型：{other:?}（期望 create_item / modify_item）"
        ))),
    }
}

fn parse_create(obj: &Map<String, Value>) -> Result<ParsedIntent, AiError> {
    let mut intent = ParsedIntent::new_create();
    intent.unit_name = get_str(obj, "unit_name");
    intent.box_name = get_str(obj, "box_name");
    intent.item_name = get_str(obj, "item_name");
    intent.item_description = get_str(obj, "item_description");
    intent.quantity = get_quantity(obj, "quantity");
    if intent.item_name.is_none() {
        return Err(AiError::ModelOutput(
            "create_item 缺少必填字段 item_name".to_string(),
        ));
    }
    Ok(intent)
}

fn parse_modify(obj: &Map<String, Value>) -> Result<ParsedIntent, AiError> {
    let mut intent = ParsedIntent::new_modify();
    intent.target_query = get_str(obj, "target_query");
    intent.new_unit_name = get_str(obj, "new_unit_name");
    intent.new_box_name = get_str(obj, "new_box_name");
    intent.new_item_name = get_str(obj, "new_item_name");
    intent.new_description = get_str(obj, "new_description");
    intent.new_quantity = get_quantity(obj, "new_quantity");
    if intent.target_query.is_none() {
        return Err(AiError::ModelOutput(
            "modify_item 缺少必填字段 target_query".to_string(),
        ));
    }
    Ok(intent)
}

/// 字符串字段：trim 后空串视为 `None`。
fn get_str(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 数量字段：接受数字或数字字符串，仅 >0 有效。
fn get_quantity(obj: &Map<String, Value>, key: &str) -> Option<i64> {
    let value = obj.get(key)?;
    match value {
        Value::Number(n) => n.as_i64().filter(|q| *q > 0),
        Value::String(s) => s.trim().parse::<i64>().ok().filter(|q| *q > 0),
        _ => None,
    }
}

fn head(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------------------
// 第①②层：直接解析 / 剥离围栏与前后文字
// ---------------------------------------------------------------------------

/// 四层容错恢复 JSON 值。
fn recover_json(text: &str) -> Option<Value> {
    // ① 直接解析
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }

    // ② 剥离 markdown 围栏 / 前后文字
    let stripped = strip_prose(text);
    if stripped != text {
        if let Ok(v) = serde_json::from_str::<Value>(&stripped) {
            return Some(v);
        }
    }

    // ③ 修复（对原文与剥离后的文本各跑一遍修复管线）
    let mut seen = std::collections::HashSet::new();
    for base in [text, stripped.as_str()] {
        for candidate in repair_candidates(base) {
            if seen.insert(candidate.clone()) {
                if let Ok(v) = serde_json::from_str::<Value>(&candidate) {
                    return Some(v);
                }
            }
        }
    }

    // ④ 彻底失败
    None
}

/// 剥离 markdown 围栏或前后解释文字，返回第一段 JSON 对象文本。
///
/// 截断（没有闭合 `}`）时返回从首个 `{` 到末尾的片段，交给修复层补全。
fn strip_prose(text: &str) -> String {
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        // 跳过语言标记行（如 ```json）
        let content_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let content = &after[content_start..];
        let fence_end = content.find("```").unwrap_or(content.len());
        return content[..fence_end].trim().to_string();
    }
    first_json_object_slice(text).unwrap_or_else(|| text.trim().to_string())
}

/// 从首个 `{` 起按括号平衡截取第一个完整 JSON 对象；
/// 未闭合时返回到末尾的截断片段（供修复层补全）。
fn first_json_object_slice(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, ch) in text[start..].char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else {
                match ch {
                    '\\' => escape = true,
                    '"' => in_string = false,
                    _ => {}
                }
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
    }
    // 未闭合：返回截断片段
    Some(text[start..].trim().to_string())
}

// ---------------------------------------------------------------------------
// 第③层：修复管线
// ---------------------------------------------------------------------------

/// 依次应用各修复步骤，返回每一步的候选文本（调用方逐个尝试解析）。
fn repair_candidates(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut cur = text.to_string();

    // 1) 中英文引号/标点归一
    let step = normalize_quotes(&cur);
    candidates.push(step.clone());
    cur = step;

    // 2) 单引号键值 → 双引号（仅在无 ASCII 双引号时尝试）
    let step = single_to_double_quotes(&cur);
    candidates.push(step.clone());
    cur = step;

    // 3) 去除对象/数组末尾多余逗号
    let step = remove_trailing_commas(&cur);
    candidates.push(step.clone());
    cur = step;

    // 4) 括号重平衡与截断补全
    let step = rebalance(&cur);
    candidates.push(step);

    candidates
}

/// 全角/花式引号与标点 → ASCII 等价物。
fn normalize_quotes(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{FF02}' => '"',
            '\u{2018}' | '\u{2019}' | '\u{FF07}' => '\'',
            '\u{FF1A}' => ':', // ：
            '\u{FF0C}' => ',', // ，
            '\u{3000}' => ' ', // 全角空格
            _ => ch,
        })
        .collect()
}

/// 单引号 JSON → 双引号。仅在文本不含 ASCII 双引号时生效，
/// 避免误伤合法内容。
fn single_to_double_quotes(text: &str) -> String {
    if text.contains('"') {
        return text.to_string();
    }
    text.replace('\'', "\"")
}

/// 去除 `}` / `]` 前的尾逗号（忽略字符串内部的逗号）。
fn remove_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escape = false;
    for (i, &ch) in chars.iter().enumerate() {
        if in_string {
            out.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            ',' => {
                // 向后找下一个非空白字符
                let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
                if matches!(next, Some('}') | Some(']')) || next.is_none() {
                    // 丢弃尾逗号
                } else {
                    out.push(ch);
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

/// 括号重平衡与截断修复：补全未闭合的字符串与括号，
/// 并去掉补全前多余的尾逗号。
fn rebalance(text: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    for ch in text.chars() {
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' => {
                if stack.last() == Some(&'}') {
                    stack.pop();
                }
            }
            ']' => {
                if stack.last() == Some(&']') {
                    stack.pop();
                }
            }
            _ => {}
        }
    }

    let mut out: String = text.trim_end().to_string();
    if in_string {
        out.push('"');
    }
    while out.ends_with(',') {
        out.pop();
    }
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(raw: &str) -> ParsedIntent {
        parse_intent_from_output(raw).unwrap_or_else(|e| panic!("应解析成功：{e:?}\n输入：{raw}"))
    }

    // ---------- 黄金测试：≥15 条真实脏输出形态 ----------

    /// 1. 标准 JSON（create）
    #[test]
    fn golden_standard_json_create() {
        let intent = parse_ok(
            r#"{"intent":"create_item","unit_name":"车库","box_name":"蓝色箱子","item_name":"电钻","item_description":"","quantity":1}"#,
        );
        assert_eq!(intent.intent, IntentKind::CreateItem);
        assert_eq!(intent.unit_name.as_deref(), Some("车库"));
        assert_eq!(intent.box_name.as_deref(), Some("蓝色箱子"));
        assert_eq!(intent.item_name.as_deref(), Some("电钻"));
        assert_eq!(intent.item_description, None);
        assert_eq!(intent.quantity, Some(1));
    }

    /// 2. 标准 JSON（modify 移动）
    #[test]
    fn golden_standard_json_modify_move() {
        let intent = parse_ok(
            r#"{"intent":"modify_item","target_query":"扳手","new_box_name":"杂物箱","new_unit_name":"厨房"}"#,
        );
        assert_eq!(intent.intent, IntentKind::ModifyItem);
        assert_eq!(intent.target_query.as_deref(), Some("扳手"));
        assert_eq!(intent.new_box_name.as_deref(), Some("杂物箱"));
        assert_eq!(intent.new_unit_name.as_deref(), Some("厨房"));
        assert_eq!(intent.new_quantity, None);
    }

    /// 3. markdown 围栏（带语言标记）
    #[test]
    fn golden_markdown_fence_with_language() {
        let intent = parse_ok(
            "```json\n{\"intent\":\"create_item\",\"item_name\":\"充电宝\",\"box_name\":\"抽屉盒\",\"quantity\":3}\n```",
        );
        assert_eq!(intent.item_name.as_deref(), Some("充电宝"));
        assert_eq!(intent.quantity, Some(3));
    }

    /// 4. markdown 围栏（无语言标记）
    #[test]
    fn golden_markdown_fence_plain() {
        let intent = parse_ok(
            "```\n{\"intent\":\"modify_item\",\"target_query\":\"羽绒服\",\"new_quantity\":2}\n```",
        );
        assert_eq!(intent.intent, IntentKind::ModifyItem);
        assert_eq!(intent.new_quantity, Some(2));
    }

    /// 5. 前后有解释文字
    #[test]
    fn golden_prose_around_json() {
        let intent = parse_ok(
            "好的，解析结果如下：{\"intent\":\"create_item\",\"item_name\":\"螺丝刀\",\"box_name\":\"工具箱\"} 希望对你有帮助！",
        );
        assert_eq!(intent.item_name.as_deref(), Some("螺丝刀"));
        assert_eq!(intent.box_name.as_deref(), Some("工具箱"));
    }

    /// 6. 模型输出多段 JSON，取第一段
    #[test]
    fn golden_multiple_objects_takes_first() {
        let intent = parse_ok(
            r#"{"intent":"create_item","item_name":"台灯","box_name":"纸箱A"} 另外也可写成 {"intent":"create_item","item_name":"错误段"}"#,
        );
        assert_eq!(intent.item_name.as_deref(), Some("台灯"));
        assert_eq!(intent.box_name.as_deref(), Some("纸箱A"));
    }

    /// 7. 截断：缺右括号
    #[test]
    fn golden_truncated_missing_brace() {
        let intent = parse_ok(r#"{"intent":"create_item","item_name":"电钻","box_name":"蓝箱""#);
        assert_eq!(intent.item_name.as_deref(), Some("电钻"));
    }

    /// 8. 截断：引号未闭合
    #[test]
    fn golden_truncated_unclosed_string() {
        let intent = parse_ok(r#"{"intent":"create_item","item_name":"手电"#);
        assert_eq!(intent.item_name.as_deref(), Some("手电"));
        assert_eq!(intent.intent, IntentKind::CreateItem);
    }

    /// 9. 字段缺失（仅必填项）
    #[test]
    fn golden_missing_optional_fields() {
        let intent = parse_ok(r#"{"intent":"create_item","item_name":"胶带"}"#);
        assert_eq!(intent.item_name.as_deref(), Some("胶带"));
        assert_eq!(intent.unit_name, None);
        assert_eq!(intent.box_name, None);
        assert_eq!(intent.quantity, None);
    }

    /// 10. 多余尾逗号
    #[test]
    fn golden_trailing_comma() {
        let intent = parse_ok(
            r#"{"intent":"create_item","item_name":"卷尺","box_name":"工具柜",}"#,
        );
        assert_eq!(intent.item_name.as_deref(), Some("卷尺"));
    }

    /// 11. 单引号
    #[test]
    fn golden_single_quotes() {
        let intent = parse_ok("{'intent':'create_item','item_name':'电池','quantity':4}");
        assert_eq!(intent.item_name.as_deref(), Some("电池"));
        assert_eq!(intent.quantity, Some(4));
    }

    /// 12. 中文（全角）引号混排
    #[test]
    fn golden_chinese_quotes() {
        let intent =
            parse_ok("{“intent”：“create_item”，“item_name”：“毛衣”，“box_name”：“衣柜收纳箱”}");
        assert_eq!(intent.item_name.as_deref(), Some("毛衣"));
        assert_eq!(intent.box_name.as_deref(), Some("衣柜收纳箱"));
    }

    /// 13. 围栏 + 截断组合
    #[test]
    fn golden_fence_plus_truncation() {
        let intent = parse_ok(
            "```json\n{\"intent\":\"modify_item\",\"target_query\":\"电钻\",\"new_box_name\":\"红箱\"\n```",
        );
        assert_eq!(intent.target_query.as_deref(), Some("电钻"));
        assert_eq!(intent.new_box_name.as_deref(), Some("红箱"));
    }

    /// 14. 数量为字符串
    #[test]
    fn golden_quantity_as_string() {
        let intent = parse_ok(r#"{"intent":"create_item","item_name":"袜子","quantity":"5"}"#);
        assert_eq!(intent.quantity, Some(5));
        // 非正数视为未提供
        let intent = parse_ok(r#"{"intent":"create_item","item_name":"袜子","quantity":0}"#);
        assert_eq!(intent.quantity, None);
    }

    /// 15. 意图别名（add / move）
    #[test]
    fn golden_intent_aliases() {
        let intent = parse_ok(r#"{"intent":"add","item_name":"雨伞","box_name":"门边桶"}"#);
        assert_eq!(intent.intent, IntentKind::CreateItem);
        let intent = parse_ok(r#"{"intent":"move","target_query":"雨伞","new_box_name":"鞋柜"}"#);
        assert_eq!(intent.intent, IntentKind::ModifyItem);
    }

    /// 16. 空白字符与换行包围
    #[test]
    fn golden_whitespace_wrapped() {
        let intent = parse_ok(
            "\n\n  {\"intent\":\"create_item\",\"item_name\":\"指甲刀\",\"box_name\":\"药箱\"}  \n",
        );
        assert_eq!(intent.item_name.as_deref(), Some("指甲刀"));
    }

    // ---------- 错误路径 ----------

    /// 17. 空输入
    #[test]
    fn error_empty_input() {
        let err = parse_intent_from_output("   ").unwrap_err();
        assert!(matches!(err, AiError::ModelOutput(_)));
    }

    /// 18. 完全无法解析的乱输出（错误携带原始文本摘要）
    #[test]
    fn error_garbage_output() {
        let err = parse_intent_from_output("抱歉，我无法理解你的要求。").unwrap_err();
        match err {
            AiError::ModelOutput(msg) => assert!(msg.contains("抱歉")),
            other => panic!("应为 ModelOutput，实际 {other:?}"),
        }
    }

    /// 19. 未知意图类型
    #[test]
    fn error_unknown_intent() {
        let err = parse_intent_from_output(r#"{"intent":"delete_item","item_name":"x"}"#)
            .unwrap_err();
        assert!(matches!(err, AiError::ModelOutput(_)));
    }

    /// 20. create 缺 item_name / modify 缺 target_query
    #[test]
    fn error_missing_required_fields() {
        let err = parse_intent_from_output(r#"{"intent":"create_item","box_name":"箱子"}"#)
            .unwrap_err();
        assert!(matches!(err, AiError::ModelOutput(_)));
        let err =
            parse_intent_from_output(r#"{"intent":"modify_item","new_quantity":3}"#).unwrap_err();
        assert!(matches!(err, AiError::ModelOutput(_)));
    }

    /// 21. 顶层不是对象
    #[test]
    fn error_non_object_top_level() {
        assert!(matches!(
            parse_intent_from_output("[1,2,3]"),
            Err(AiError::ModelOutput(_))
        ));
    }

    // ---------- 纯函数细节 ----------

    #[test]
    fn strip_prose_fence_extraction() {
        assert_eq!(
            strip_prose("```json\n{\"a\":1}\n```"),
            "{\"a\":1}"
        );
        // 未闭合围栏 → 取到末尾
        assert_eq!(strip_prose("```\n{\"a\":1"), "{\"a\":1");
    }

    #[test]
    fn first_json_object_slice_balanced() {
        let text = "前言 {\"a\":{\"b\":1}} 后记 {\"c\":2}";
        assert_eq!(
            first_json_object_slice(text).unwrap(),
            "{\"a\":{\"b\":1}}"
        );
        // 字符串内的括号不影响平衡
        let text = "x{\"k\":\"}\"}y";
        assert_eq!(first_json_object_slice(text).unwrap(), "{\"k\":\"}\"}");
        // 无对象
        assert!(first_json_object_slice("没有对象").is_none());
    }

    #[test]
    fn remove_trailing_commas_keeps_inner() {
        assert_eq!(
            remove_trailing_commas(r#"{"a":1,"b":[1,2,],"c":"x,y",}"#),
            r#"{"a":1,"b":[1,2],"c":"x,y"}"#
        );
        // 字符串内的逗号不受影响
        assert_eq!(
            remove_trailing_commas(r#"{"s":"a,}"#),
            r#"{"s":"a,}"#
        );
    }

    #[test]
    fn rebalance_closes_nested() {
        assert_eq!(
            rebalance(r#"{"a":{"b":[1,2"#),
            r#"{"a":{"b":[1,2]}}"#
        );
        // 未闭合字符串 + 括号
        assert_eq!(rebalance(r#"{"a":"x"#), r#"{"a":"x"}"#);
    }

    #[test]
    fn normalize_quotes_maps_cjk_punctuation() {
        assert_eq!(normalize_quotes("{“k”：“v”，}"), "{\"k\":\"v\",}");
        assert_eq!(normalize_quotes("‘a’"), "'a'");
    }

    #[test]
    fn single_to_double_only_without_ascii_quotes() {
        assert_eq!(
            single_to_double_quotes("{'a':'b'}"),
            "{\"a\":\"b\"}"
        );
        // 已有双引号时不改动（避免破坏内容）
        assert_eq!(single_to_double_quotes("{\"a\":'b'}"), "{\"a\":'b'}");
    }

    #[test]
    fn parsed_intent_defaults() {
        let create = ParsedIntent::new_create();
        assert_eq!(create.intent, IntentKind::CreateItem);
        assert!(create.item_name.is_none());
        let modify = ParsedIntent::new_modify();
        assert_eq!(modify.intent, IntentKind::ModifyItem);
    }

    /// 守护：系统提示词内嵌的 schema 与修复重试提示使用的常量保持一致。
    #[test]
    fn system_prompt_contains_schema_constants() {
        assert!(PARSE_SYSTEM_PROMPT.contains(CREATE_SCHEMA_JSON));
        assert!(PARSE_SYSTEM_PROMPT.contains(MODIFY_SCHEMA_JSON));
    }

    // ---------- 修复重试提示 ----------

    #[test]
    fn repair_prompt_create_covers_schema_and_context() {
        let prompt = build_repair_retry_prompt(
            IntentKind::CreateItem,
            "把电钻放进车库的蓝色箱子",
            "抱歉，我无法完成这个请求。",
            "无法从模型输出中解析出 JSON",
        );
        // 中文上下文原样保留
        assert!(prompt.contains("把电钻放进车库的蓝色箱子"));
        assert!(prompt.contains("抱歉，我无法完成这个请求。"));
        assert!(prompt.contains("无法从模型输出中解析出 JSON"));
        // create 场景以 create schema 在前，且两种 schema 都附上
        let create_pos = prompt.find(CREATE_SCHEMA_JSON).unwrap();
        let modify_pos = prompt.find(MODIFY_SCHEMA_JSON).unwrap();
        assert!(create_pos < modify_pos);
        // 要求只输出合法 JSON
        assert!(prompt.contains("只输出一个纯 JSON 对象"));
    }

    #[test]
    fn repair_prompt_modify_leads_with_modify_schema() {
        let prompt = build_repair_retry_prompt(
            IntentKind::ModifyItem,
            "把羽绒服的数量改成2",
            "```json",
            "顶层不是 JSON 对象",
        );
        let create_pos = prompt.find(CREATE_SCHEMA_JSON).unwrap();
        let modify_pos = prompt.find(MODIFY_SCHEMA_JSON).unwrap();
        assert!(modify_pos < create_pos);
        assert!(prompt.contains("把羽绒服的数量改成2"));
        assert!(prompt.contains("顶层不是 JSON 对象"));
    }

    #[test]
    fn repair_prompt_truncates_long_summary_by_chars() {
        // 超长错误输出被按字符截取：保留前 200 字符，不出现第 201 个
        let long_output: String = "收".repeat(500);
        let prompt = build_repair_retry_prompt(
            IntentKind::CreateItem,
            "毛衣装箱",
            &long_output,
            "原因",
        );
        assert!(prompt.contains(&"收".repeat(REPAIR_SUMMARY_MAX_CHARS)));
        assert!(!prompt.contains(&"收".repeat(REPAIR_SUMMARY_MAX_CHARS + 1)));
    }

    #[test]
    fn repair_prompt_truncation_is_utf8_safe() {
        // 多字节中文混排下按 chars() 截取不切断字符（不会 panic 也不乱码）
        let mixed: String = "收纳箱柜".repeat(120); // 480 字符
        let expected_head: String = mixed.chars().take(REPAIR_SUMMARY_MAX_CHARS).collect();
        assert_eq!(expected_head.chars().count(), REPAIR_SUMMARY_MAX_CHARS);
        let prompt = build_repair_retry_prompt(
            IntentKind::ModifyItem,
            "挪一下",
            &mixed,
            "原因",
        );
        assert!(prompt.contains(&expected_head));
    }
}
