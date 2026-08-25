//! AI HTTP 客户端。
//!
//! - [`AiTransport`] trait 隔离网络：对话（chat）与批量向量（embed）；
//! - [`HttpAiTransport`] 为 reqwest（blocking + rustls）实现；
//! - 请求体构造、响应解析、错误分类、重试策略均为纯函数，可单测；
//! - 超时分级：对话 60s、向量 30s（可按场景缩短）；
//! - 仅网络类错误重试（默认 2 次，退避 1s / 3s）。

use std::time::Duration;

use serde_json::{json, Value};

use crate::core::ai::config::{AiConfig, AiProvider};

/// 对话请求超时。
pub const CHAT_TIMEOUT: Duration = Duration::from_secs(60);
/// 向量请求超时。
pub const EMBED_TIMEOUT: Duration = Duration::from_secs(30);
/// 网络错误重试退避序列（重试 2 次：等 1s、再等 3s）。
pub const RETRY_BACKOFFS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

/// AI 调用错误（core 层）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    /// 未配置服务地址。
    NotConfigured,
    /// 网络层失败：连接被拒 / 超时 / DNS 失败等（可重试）。
    Network(String),
    /// 服务可达但返回 HTTP 错误状态（如 404 模型不存在、500 内部错误）。
    Server(String),
    /// HTTP 200 但响应体无法解析或结构不符（模型/服务端内容问题）。
    ModelOutput(String),
}

impl AiError {
    /// 是否为网络类错误（唯一可重试的类别）。
    pub fn is_network(&self) -> bool {
        matches!(self, AiError::Network(_))
    }
}

/// 网络抽象：对话 + 批量向量。测试中以 mock 替换。
pub trait AiTransport {
    /// 单轮对话，返回模型文本内容。
    fn chat(&self, config: &AiConfig, system: &str, user: &str) -> Result<String, AiError>;

    /// 批量生成向量；成功时返回与 `inputs` 等长的二维向量。
    fn embed(&self, config: &AiConfig, inputs: &[String]) -> Result<Vec<Vec<f32>>, AiError>;
}

// ---------------------------------------------------------------------------
// 纯函数：URL / 请求体 / 响应解析
// ---------------------------------------------------------------------------

/// 对话端点。
pub fn chat_url(config: &AiConfig) -> String {
    let base = config.normalized_base_url();
    match config.provider {
        AiProvider::Ollama => format!("{base}/api/chat"),
        AiProvider::OpenAi => format!("{base}/v1/chat/completions"),
    }
}

/// 向量端点。
pub fn embed_url(config: &AiConfig) -> String {
    let base = config.normalized_base_url();
    match config.provider {
        AiProvider::Ollama => format!("{base}/api/embed"),
        AiProvider::OpenAi => format!("{base}/v1/embeddings"),
    }
}

/// 构造对话请求体。
///
/// - Ollama：`format: "json"` 要求输出纯 JSON，`stream: false`；
/// - OpenAI 兼容：`response_format: json_object`。
pub fn build_chat_body(config: &AiConfig, system: &str, user: &str) -> Value {
    let messages = json!([
        { "role": "system", "content": system },
        { "role": "user", "content": user },
    ]);
    match config.provider {
        AiProvider::Ollama => json!({
            "model": config.chat_model,
            "messages": messages,
            "format": "json",
            "stream": false,
            "options": { "temperature": 0.1 },
        }),
        AiProvider::OpenAi => json!({
            "model": config.chat_model,
            "messages": messages,
            "response_format": { "type": "json_object" },
            "temperature": 0.1,
        }),
    }
}

/// 构造向量请求体：`input` 一律为字符串数组（批量）。
pub fn build_embed_body(config: &AiConfig, inputs: &[String]) -> Value {
    json!({
        "model": config.embed_model,
        "input": inputs,
    })
}

/// 从对话响应体提取模型文本内容。
pub fn extract_chat_content(provider: AiProvider, body: &Value) -> Result<String, AiError> {
    let content = match provider {
        AiProvider::Ollama => body.pointer("/message/content"),
        AiProvider::OpenAi => body.pointer("/choices/0/message/content"),
    };
    let text = content
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AiError::ModelOutput("响应中缺少模型文本内容（预期 message.content）".to_string())
        })?;
    Ok(text.to_string())
}

/// 从向量响应体提取二维向量。
///
/// - Ollama `/api/embed`：`embeddings: [[...], ...]`，顺序与输入一致；
/// - OpenAI：`data: [{index, embedding}, ...]`，按 `index` 排序。
/// 数量与 `expected` 不一致时报 [`AiError::ModelOutput`]。
pub fn extract_embeddings(
    provider: AiProvider,
    body: &Value,
    expected: usize,
) -> Result<Vec<Vec<f32>>, AiError> {
    let invalid = |detail: &str| AiError::ModelOutput(format!("向量响应解析失败：{detail}"));

    let mut result: Vec<(usize, Vec<f32>)> = Vec::new();
    match provider {
        AiProvider::Ollama => {
            let rows = body
                .get("embeddings")
                .and_then(|v| v.as_array())
                .ok_or_else(|| invalid("缺少 embeddings 数组"))?;
            for (i, row) in rows.iter().enumerate() {
                let vec = parse_float_row(row).ok_or_else(|| invalid("向量含非数值"))?;
                result.push((i, vec));
            }
        }
        AiProvider::OpenAi => {
            let rows = body
                .get("data")
                .and_then(|v| v.as_array())
                .ok_or_else(|| invalid("缺少 data 数组"))?;
            for row in rows {
                let index = row
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(u64::MAX) as usize;
                let embedding = row
                    .get("embedding")
                    .ok_or_else(|| invalid("缺少 embedding 字段"))?;
                result.push((
                    index,
                    parse_float_row(embedding).ok_or_else(|| invalid("向量含非数值"))?,
                ));
            }
            result.sort_by_key(|(i, _)| *i);
        }
    }

    if result.len() != expected {
        return Err(AiError::ModelOutput(format!(
            "向量数量不符：期望 {expected} 条，实际 {} 条",
            result.len()
        )));
    }
    if result.iter().any(|(_, v)| v.is_empty()) {
        return Err(invalid("存在空向量"));
    }
    Ok(result.into_iter().map(|(_, v)| v).collect())
}

fn parse_float_row(row: &Value) -> Option<Vec<f32>> {
    let arr = row.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        out.push(v.as_f64()? as f32);
    }
    Some(out)
}

/// HTTP 错误状态 → [`AiError::Server`]（附带状态码与截断的响应体）。
pub fn classify_http_error(status: u16, body: &str) -> AiError {
    let snippet: String = body.trim().chars().take(200).collect();
    let detail = if snippet.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}：{snippet}")
    };
    AiError::Server(detail)
}

/// 仅对网络类错误重试：首次 + `backoffs.len()` 次重试。
///
/// `sleep` 注入以便测试；生产传 [`std::thread::sleep`]。
pub fn retry_on_network<F, T, S>(mut op: F, backoffs: &[Duration], mut sleep: S) -> Result<T, AiError>
where
    F: FnMut() -> Result<T, AiError>,
    S: FnMut(Duration),
{
    let mut attempt = 0usize;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if e.is_network() && attempt < backoffs.len() => {
                sleep(backoffs[attempt]);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// reqwest（blocking + rustls）实现
// ---------------------------------------------------------------------------

/// 基于 reqwest 的真实传输层。
///
/// 使用 blocking 客户端（内部自带 tokio 运行时），避免与
/// flutter_rust_bridge 异步线程池的运行时耦合。
#[derive(Debug, Clone)]
pub struct HttpAiTransport {
    chat_timeout: Duration,
    embed_timeout: Duration,
    max_retries: usize,
}

impl Default for HttpAiTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpAiTransport {
    /// 默认：对话 60s / 向量 30s / 网络错误重试 2 次。
    pub fn new() -> Self {
        HttpAiTransport {
            chat_timeout: CHAT_TIMEOUT,
            embed_timeout: EMBED_TIMEOUT,
            max_retries: RETRY_BACKOFFS.len(),
        }
    }

    /// 覆盖向量超时（如搜索查询向量希望更快失败降级）。
    pub fn with_embed_timeout(mut self, timeout: Duration) -> Self {
        self.embed_timeout = timeout;
        self
    }

    /// 覆盖重试次数（0 = 不重试）。
    pub fn with_max_retries(mut self, retries: usize) -> Self {
        self.max_retries = retries;
        self
    }

    fn backoffs(&self) -> &[Duration] {
        &RETRY_BACKOFFS[..self.max_retries.min(RETRY_BACKOFFS.len())]
    }

    /// 发送一次 POST 并解析 JSON 响应（不含重试）。
    fn post_once(
        &self,
        config: &AiConfig,
        url: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, AiError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| AiError::Network(format!("HTTP 客户端初始化失败：{e}")))?;

        let mut request = client.post(url).json(body);
        if config.provider == AiProvider::OpenAi && !config.api_key.trim().is_empty() {
            request = request.bearer_auth(config.api_key.trim());
        }

        let response = request
            .send()
            .map_err(|e| AiError::Network(e.to_string()))?;

        let status = response.status();
        let text = response.text().map_err(|e| AiError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(classify_http_error(status.as_u16(), &text));
        }
        serde_json::from_str(&text).map_err(|e| {
            AiError::ModelOutput(format!("响应不是合法 JSON：{e}；原文前 120 字：{}", &text[..text.len().min(120)]))
        })
    }
}

impl AiTransport for HttpAiTransport {
    fn chat(&self, config: &AiConfig, system: &str, user: &str) -> Result<String, AiError> {
        if !config.is_configured() {
            return Err(AiError::NotConfigured);
        }
        let url = chat_url(config);
        let body = build_chat_body(config, system, user);
        let timeout = self.chat_timeout;
        retry_on_network(
            || {
                let value = self.post_once(config, &url, &body, timeout)?;
                extract_chat_content(config.provider, &value)
            },
            self.backoffs(),
            std::thread::sleep,
        )
    }

    fn embed(&self, config: &AiConfig, inputs: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
        if !config.is_configured() {
            return Err(AiError::NotConfigured);
        }
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let url = embed_url(config);
        let body = build_embed_body(config, inputs);
        let timeout = self.embed_timeout;
        let expected = inputs.len();
        retry_on_network(
            || {
                let value = self.post_once(config, &url, &body, timeout)?;
                extract_embeddings(config.provider, &value, expected)
            },
            self.backoffs(),
            std::thread::sleep,
        )
    }
}

/// core 层 [`AiError`] → 全局 [`crate::core::error::FinditError`]。
pub fn ai_error_to_findit(error: AiError) -> crate::core::error::FinditError {
    use crate::core::error::FinditError;
    match error {
        AiError::NotConfigured => FinditError::AiNotConfigured("请先在设置中填写 AI 服务地址".to_string()),
        AiError::Network(detail) => FinditError::AiUnreachable(detail),
        AiError::Server(detail) => FinditError::AiModelOutput(detail),
        AiError::ModelOutput(detail) => FinditError::AiModelOutput(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ollama_config() -> AiConfig {
        AiConfig {
            provider: AiProvider::Ollama,
            base_url: "http://localhost:11434/".to_string(),
            api_key: String::new(),
            chat_model: "qwen3:4b".to_string(),
            embed_model: "nomic-embed-text".to_string(),
        }
    }

    fn openai_config() -> AiConfig {
        AiConfig {
            provider: AiProvider::OpenAi,
            base_url: "https://api.example.com".to_string(),
            api_key: "sk-x".to_string(),
            chat_model: "gpt-4o-mini".to_string(),
            embed_model: "text-embedding-3-small".to_string(),
        }
    }

    #[test]
    fn urls_follow_provider_conventions() {
        assert_eq!(chat_url(&ollama_config()), "http://localhost:11434/api/chat");
        assert_eq!(embed_url(&ollama_config()), "http://localhost:11434/api/embed");
        assert_eq!(
            chat_url(&openai_config()),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            embed_url(&openai_config()),
            "https://api.example.com/v1/embeddings"
        );
    }

    #[test]
    fn chat_body_ollama_uses_format_json() {
        let body = build_chat_body(&ollama_config(), "sys", "hello");
        assert_eq!(body["model"], "qwen3:4b");
        assert_eq!(body["format"], "json");
        assert_eq!(body["stream"], false);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["content"], "hello");
    }

    #[test]
    fn chat_body_openai_uses_response_format() {
        let body = build_chat_body(&openai_config(), "sys", "hello");
        assert_eq!(body["response_format"]["type"], "json_object");
        assert!(body.get("format").is_none());
    }

    #[test]
    fn embed_body_uses_string_array_input() {
        let inputs = vec!["甲".to_string(), "乙".to_string()];
        let body = build_embed_body(&ollama_config(), &inputs);
        assert_eq!(body["model"], "nomic-embed-text");
        assert_eq!(body["input"], json!(["甲", "乙"]));
    }

    #[test]
    fn extract_chat_content_ollama() {
        let body = json!({ "message": { "role": "assistant", "content": " {\"intent\":\"create_item\"} " } });
        let text = extract_chat_content(AiProvider::Ollama, &body).unwrap();
        assert_eq!(text, "{\"intent\":\"create_item\"}");
    }

    #[test]
    fn extract_chat_content_openai() {
        let body = json!({ "choices": [ { "message": { "content": "ok" } } ] });
        assert_eq!(extract_chat_content(AiProvider::OpenAi, &body).unwrap(), "ok");
    }

    #[test]
    fn extract_chat_content_missing_is_model_error() {
        let err = extract_chat_content(AiProvider::Ollama, &json!({})).unwrap_err();
        assert!(matches!(err, AiError::ModelOutput(_)));
        let err = extract_chat_content(AiProvider::OpenAi, &json!({"choices": []})).unwrap_err();
        assert!(matches!(err, AiError::ModelOutput(_)));
        // 空内容也算异常
        let body = json!({ "message": { "content": "   " } });
        assert!(matches!(
            extract_chat_content(AiProvider::Ollama, &body),
            Err(AiError::ModelOutput(_))
        ));
    }

    #[test]
    fn extract_embeddings_ollama_preserves_order() {
        let body = json!({ "embeddings": [[0.1, 0.2], [0.3, 0.4]] });
        let vecs = extract_embeddings(AiProvider::Ollama, &body, 2).unwrap();
        assert_eq!(vecs, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[test]
    fn extract_embeddings_openai_sorts_by_index() {
        let body = json!({
            "data": [
                { "index": 1, "embedding": [2.0] },
                { "index": 0, "embedding": [1.0] },
            ]
        });
        let vecs = extract_embeddings(AiProvider::OpenAi, &body, 2).unwrap();
        assert_eq!(vecs, vec![vec![1.0], vec![2.0]]);
    }

    #[test]
    fn extract_embeddings_count_mismatch_rejected() {
        let body = json!({ "embeddings": [[0.1]] });
        let err = extract_embeddings(AiProvider::Ollama, &body, 2).unwrap_err();
        assert!(matches!(err, AiError::ModelOutput(_)));
    }

    #[test]
    fn extract_embeddings_invalid_shapes_rejected() {
        assert!(matches!(
            extract_embeddings(AiProvider::Ollama, &json!({}), 1),
            Err(AiError::ModelOutput(_))
        ));
        assert!(matches!(
            extract_embeddings(AiProvider::Ollama, &json!({"embeddings": [["x"]]}), 1),
            Err(AiError::ModelOutput(_))
        ));
        assert!(matches!(
            extract_embeddings(AiProvider::Ollama, &json!({"embeddings": [[]]}), 1),
            Err(AiError::ModelOutput(_))
        ));
    }

    #[test]
    fn classify_http_error_includes_status_and_snippet() {
        let err = classify_http_error(404, "model 'x' not found");
        assert_eq!(err, AiError::Server("HTTP 404：model 'x' not found".to_string()));
        let err = classify_http_error(500, "");
        assert_eq!(err, AiError::Server("HTTP 500".to_string()));
        assert!(!err.is_network());
    }

    #[test]
    fn retry_on_network_retries_only_network_errors() {
        // 前两次网络错误 → 第三次成功：共 3 次调用、2 次 sleep。
        let mut calls = 0;
        let mut sleeps: Vec<Duration> = Vec::new();
        let result = retry_on_network(
            || {
                calls += 1;
                if calls < 3 {
                    Err(AiError::Network("connect refused".into()))
                } else {
                    Ok(42)
                }
            },
            &RETRY_BACKOFFS,
            |d| sleeps.push(d),
        );
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 3);
        assert_eq!(sleeps, vec![Duration::from_secs(1), Duration::from_secs(3)]);

        // 服务端错误不重试。
        let mut calls = 0;
        let result: Result<(), AiError> = retry_on_network(
            || {
                calls += 1;
                Err(AiError::Server("HTTP 500".into()))
            },
            &RETRY_BACKOFFS,
            |_| {},
        );
        assert!(matches!(result, Err(AiError::Server(_))));
        assert_eq!(calls, 1);

        // 网络错误耗尽重试次数后返回最后一次错误。
        let mut calls = 0;
        let result: Result<(), AiError> = retry_on_network(
            || {
                calls += 1;
                Err(AiError::Network("timeout".into()))
            },
            &RETRY_BACKOFFS,
            |_| {},
        );
        assert!(matches!(result, Err(AiError::Network(_))));
        assert_eq!(calls, 3);

        // 空退避序列 = 不重试。
        let mut calls = 0;
        let result: Result<(), AiError> = retry_on_network(
            || {
                calls += 1;
                Err(AiError::Network("x".into()))
            },
            &[],
            |_| {},
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn ai_error_classification_flags() {
        assert!(AiError::Network("x".into()).is_network());
        assert!(!AiError::Server("x".into()).is_network());
        assert!(!AiError::ModelOutput("x".into()).is_network());
        assert!(!AiError::NotConfigured.is_network());
    }

    #[test]
    fn ai_error_maps_to_findit_error() {
        use crate::core::error::FinditError;
        assert!(matches!(
            ai_error_to_findit(AiError::NotConfigured),
            FinditError::AiNotConfigured(_)
        ));
        assert!(matches!(
            ai_error_to_findit(AiError::Network("t".into())),
            FinditError::AiUnreachable(_)
        ));
        assert!(matches!(
            ai_error_to_findit(AiError::Server("t".into())),
            FinditError::AiModelOutput(_)
        ));
        assert!(matches!(
            ai_error_to_findit(AiError::ModelOutput("t".into())),
            FinditError::AiModelOutput(_)
        ));
    }
}
