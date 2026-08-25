//! AI API 薄壳：配置、探活、一句话解析与应用、查询向量、回填与重建。
//!
//! 所有业务逻辑在 `core::ai`；本层只做转发 + 全局状态编排。
//! 锁边界铁律：网络调用（`transport.embed`）绝不发生在 `with_conn`
//! 持锁闭包内——回填按「锁内取一批待办 → 释放锁做网络 → 锁内写回」三段执行。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::frb_generated::StreamSink;

use crate::api::model::{AiStatus, AiTestResult, EmbedProgress, EmbedProgressSummary};
use crate::core::ai::apply::{apply_create, apply_modify, ModifyResult, QuickAddResult};
use crate::core::ai::client::{ai_error_to_findit, AiError, AiTransport, HttpAiTransport};
use crate::core::ai::config::{load_ai_config, AiConfig, KEY_EMBEDDED_DIM, KEY_EMBEDDED_MODEL};
use crate::core::ai::embed::{
    apply_backfill, clear_all_embeddings, count_items, count_pending_embeddings,
    pending_item_texts, DEFAULT_BATCH_SIZE,
};
use crate::core::ai::parse::{
    build_repair_retry_prompt, parse_intent_from_output, IntentKind, ParsedIntent,
    PARSE_SYSTEM_PROMPT,
};
use crate::core::db::with_conn;
use crate::core::error::FinditError;
use crate::core::repo::settings::get_setting;

/// 探活结果缓存有效期：`get_ai_status` 不每次重探。
const TEST_CACHE_TTL: Duration = Duration::from_secs(60);
/// 搜索查询向量的短超时：失败即降级为关键词搜索，不阻塞搜索结果呈现。
const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(3);

static LAST_TEST: Mutex<Option<(Instant, AiTestResult)>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 读取 AI 配置（缺失项以默认值补齐）。
pub async fn get_ai_config() -> Result<AiConfig, FinditError> {
    with_conn(load_ai_config)
}

/// 保存 AI 配置到 app_settings。服务地址不允许为空。
pub async fn save_ai_config(config: AiConfig) -> Result<bool, FinditError> {
    if config.base_url.trim().is_empty() {
        return Err(FinditError::Validation("服务地址不能为空".to_string()));
    }
    if config.has_separate_embed_service() && config.embed_base_url.trim().is_empty() {
        return Err(FinditError::Validation("向量服务地址不能为空".to_string()));
    }
    with_conn(|conn| crate::core::ai::config::save_ai_config(conn, &config)).map(|_| true)
}

// ---------------------------------------------------------------------------
// 探活 / 状态
// ---------------------------------------------------------------------------

fn ai_error_message(error: AiError) -> String {
    match error {
        AiError::NotConfigured => "未配置服务地址".to_string(),
        AiError::Network(detail) => format!("不可达：{detail}"),
        AiError::Server(detail) => format!("服务返回错误：{detail}"),
        AiError::ModelOutput(detail) => format!("输出异常：{detail}"),
    }
}

/// 测试连接：对话与向量各探一次，返回各自可用性与错误信息。
/// 结果缓存 60 秒供 `get_ai_status` 复用。
pub async fn test_ai_connection() -> Result<AiTestResult, FinditError> {
    let config = with_conn(load_ai_config)?;
    let mut result = AiTestResult {
        chat_ok: false,
        chat_message: String::new(),
        embed_ok: false,
        embed_message: String::new(),
    };

    if !config.is_configured() {
        result.chat_message = "未配置服务地址，请先在设置中填写".to_string();
        result.embed_message = result.chat_message.clone();
        cache_test_result(result.clone());
        return Ok(result);
    }

    let transport = HttpAiTransport::new();
    match transport.chat(&config, "你是探活助手。", "请只输出 JSON：{\"ok\":true}") {
        Ok(_) => {
            result.chat_ok = true;
            result.chat_message = format!("对话正常（模型：{}）", config.chat_model);
        }
        Err(e) => result.chat_message = ai_error_message(e),
    }
    // 向量测试使用有效向量配置（可能独立配置，也可能回退对话配置）。
    if !config.is_configured() && !config.has_separate_embed_service() {
        result.embed_message = "未配置向量服务地址".to_string();
    } else {
        match transport.embed(&config, &["探活测试".to_string()]) {
            Ok(_) => {
                result.embed_ok = true;
                result.embed_message = format!("向量正常（模型：{}）", config.embed_model);
            }
            Err(e) => result.embed_message = ai_error_message(e),
        }
    }

    cache_test_result(result.clone());
    Ok(result)
}

fn cache_test_result(result: AiTestResult) {
    let mut guard = LAST_TEST.lock().expect("探活缓存互斥锁中毒");
    *guard = Some((Instant::now(), result));
}

fn fresh_cached_test() -> Option<(AiTestResult, i64)> {
    let guard = LAST_TEST.lock().expect("探活缓存互斥锁中毒");
    guard.as_ref().and_then(|(at, result)| {
        let age = at.elapsed();
        if age <= TEST_CACHE_TTL {
            Some((result.clone(), age.as_secs() as i64))
        } else {
            None
        }
    })
}

/// AI 总体状态：配置概要 + 缓存探活结果（60 秒内不重探）。
pub async fn get_ai_status() -> Result<AiStatus, FinditError> {
    let config = with_conn(load_ai_config)?;
    let (embedded_model, embedded_dim, pending) = with_conn(|conn| {
        let model = get_setting(conn, KEY_EMBEDDED_MODEL)?;
        let dim = get_setting(conn, KEY_EMBEDDED_DIM)?
            .and_then(|s| s.trim().parse::<i64>().ok());
        let pending = count_pending_embeddings(conn)?;
        Ok((model, dim, pending))
    })?;

    let cached = fresh_cached_test();
    let embed_provider = config.effective_embed_provider();
    let embed_base_url = config.effective_embed_base_url();
    Ok(AiStatus {
        configured: config.is_configured(),
        provider: config.provider,
        base_url: config.base_url,
        chat_model: config.chat_model,
        embed_model: config.embed_model,
        embed_provider,
        embed_base_url,
        embedded_model,
        embedded_dim,
        pending_embeddings: pending,
        last_chat_ok: cached.as_ref().map(|(c, _)| c.chat_ok),
        last_embed_ok: cached.as_ref().map(|(c, _)| c.embed_ok),
        cache_age_secs: cached.map(|(_, age)| age),
    })
}

// ---------------------------------------------------------------------------
// 一句话解析与应用
// ---------------------------------------------------------------------------

fn parse_via_ai(text: &str, kind: IntentKind) -> Result<ParsedIntent, FinditError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(FinditError::Validation("请输入要解析的内容".to_string()));
    }
    let config = with_conn(load_ai_config)?;
    if !config.is_configured() {
        return Err(FinditError::AiNotConfigured(
            "请先在设置中填写 AI 服务地址".to_string(),
        ));
    }
    let transport = HttpAiTransport::new();
    let raw = transport
        .chat(&config, PARSE_SYSTEM_PROMPT, text)
        .map_err(ai_error_to_findit)?;
    match parse_intent_from_output(&raw) {
        Ok(intent) => Ok(intent),
        // 四层容错第 4 层：模型输出类错误时携带错误信息对同一 transport 重试恰好 1 次。
        // 网络 / 服务端错误保持既有策略不重试。全程不持全局数据库锁。
        Err(AiError::ModelOutput(detail)) => {
            let repair = build_repair_retry_prompt(kind, text, &raw, &detail);
            let retried = transport
                .chat(&config, PARSE_SYSTEM_PROMPT, &repair)
                .map_err(ai_error_to_findit)?;
            parse_intent_from_output(&retried).map_err(ai_error_to_findit)
        }
        Err(other) => Err(ai_error_to_findit(other)),
    }
}

/// 一句话 → 结构化意图（建档或修改，由模型判断）。
pub async fn parse_quick_add(text: String) -> Result<ParsedIntent, FinditError> {
    parse_via_ai(&text, IntentKind::CreateItem)
}

/// 一句话 → 修改意图（与 `parse_quick_add` 同一解析链路）。
pub async fn parse_ai_modify(text: String) -> Result<ParsedIntent, FinditError> {
    parse_via_ai(&text, IntentKind::ModifyItem)
}

/// 确认建档：单元/箱按名称自动创建或复用同名实体（单事务）。
pub async fn apply_quick_add(intent: ParsedIntent) -> Result<QuickAddResult, FinditError> {
    with_conn(|conn| apply_create(conn, &intent))
}

/// 确认修改：定位目标物品并应用变更（单事务）。
pub async fn apply_ai_modify(intent: ParsedIntent) -> Result<ModifyResult, FinditError> {
    with_conn(|conn| apply_modify(conn, &intent))
}

// ---------------------------------------------------------------------------
// 向量：查询向量 / 回填 / 重建
// ---------------------------------------------------------------------------

/// 生成搜索查询向量。未配置或任何失败均返回 `Ok(None)`，
/// 让上层降级为纯关键词搜索。
pub async fn generate_query_embedding(text: String) -> Result<Option<Vec<f32>>, FinditError> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }
    let config = with_conn(load_ai_config)?;
    if !config.is_configured() {
        return Ok(None);
    }
    let transport = HttpAiTransport::new()
        .with_embed_timeout(QUERY_EMBED_TIMEOUT)
        .with_max_retries(0);
    match transport.embed(&config, &[text]) {
        Ok(mut vecs) => Ok(vecs.pop()),
        Err(_) => Ok(None),
    }
}

/// 补齐待处理向量：扫描 `embedding IS NULL` 的物品，分批写入。
/// 返回本次处理条数。
///
/// 每轮三段式：锁内取一批待处理文本 → 释放锁后做网络调用 →
/// 锁内写回（写回前重查维度一致性）。网络期间全局锁不持有，
/// 不会冻结其它数据库操作；中断后重跑即可继续（以数据库为待办源）。
pub async fn backfill_pending_embeddings() -> Result<i32, FinditError> {
    let config = with_conn(load_ai_config)?;
    if !config.is_configured() {
        return Err(FinditError::AiNotConfigured(
            "请先在设置中填写 AI 服务地址".to_string(),
        ));
    }
    let transport = HttpAiTransport::new();
    let total = with_conn(count_items)?;
    let max_rounds = (total as usize) / DEFAULT_BATCH_SIZE + 4;

    let mut processed = 0i32;
    for _ in 0..max_rounds {
        let n = backfill_one_round(&transport, &config)?;
        if n == 0 {
            break;
        }
        processed += n;
    }
    Ok(processed)
}

/// 单轮三段式回填，返回本轮写入条数（0 = 已无待处理）。
///
/// 网络调用位于两段 `with_conn` 之间，绝不持锁。
fn backfill_one_round(transport: &HttpAiTransport, config: &AiConfig) -> Result<i32, FinditError> {
    // 第一段（锁内）：取一批待处理文本，随即释放锁。
    let pending = with_conn(|conn| pending_item_texts(conn, DEFAULT_BATCH_SIZE))?;
    if pending.is_empty() {
        return Ok(0);
    }

    // 第二段（锁外）：网络调用。
    let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();
    let vecs = transport
        .embed(config, &texts)
        .map_err(ai_error_to_findit)?;

    // 第三段（锁内）：写回（内部重查维度一致性）。
    with_conn(|conn| apply_backfill(conn, &pending, vecs, &config.embed_model))
}

/// 重建全部向量：清空后经 [`StreamSink`] 流式推送进度（done/total）。
/// Dart 端监听流；函数最终返回处理汇总。
/// 与 [`backfill_pending_embeddings`] 同样按三段式执行，网络调用不持锁。
pub fn rebuild_embeddings(sink: StreamSink<EmbedProgress>) -> Result<EmbedProgressSummary, FinditError> {
    let config = with_conn(load_ai_config)?;
    if !config.is_configured() {
        return Err(FinditError::AiNotConfigured(
            "请先在设置中填写 AI 服务地址".to_string(),
        ));
    }

    let total = with_conn(|conn| {
        let total = count_items(conn)? as i32;
        clear_all_embeddings(conn)?;
        Ok(total)
    })?;
    let _ = sink.add(EmbedProgress { done: 0, total });

    let transport = HttpAiTransport::new();
    let max_rounds = (total as usize) / DEFAULT_BATCH_SIZE + 4;
    let mut processed = 0i32;
    for _ in 0..max_rounds {
        let n = backfill_one_round(&transport, &config)?;
        if n == 0 {
            break;
        }
        processed += n;
        if sink.add(EmbedProgress { done: processed, total }).is_err() {
            break; // Dart 端已取消监听，提前结束。
        }
    }

    Ok(EmbedProgressSummary { processed, total })
}
