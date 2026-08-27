//! AI 服务配置：存于 `app_settings` 的键值对 + 各 Provider 默认值。
//!
//! 安全（评审 S-H2 / S-M2 / S-M4）：
//! - API Key（含向量独立 Key）落盘前经 [`crate::core::ai::keystore`]
//!   AES-256-GCM 加密（过渡方案，详见 keystore 模块说明；正式迁移系统 Keystore）；
//! - 配置了 API Key 时服务地址必须为 https（禁止明文传输密钥）；
//! - `semantic_backfill_enabled()` 为语义向量自动回填开关（默认关闭），
//!   供启动门控（lib/main.dart）与设置页读写。

use rusqlite::Connection;

use crate::core::ai::keystore::{self, Keystore};
use crate::core::db;
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo::settings::{get_setting, set_setting};

/// app_settings 键名。
pub const KEY_PROVIDER: &str = "ai_provider";
pub const KEY_BASE_URL: &str = "ai_base_url";
pub const KEY_API_KEY: &str = "ai_api_key";
pub const KEY_CHAT_MODEL: &str = "ai_chat_model";
pub const KEY_EMBED_MODEL: &str = "ai_embed_model";
/// 向量服务独立配置；为空时回退对话服务配置。
pub const KEY_EMBED_PROVIDER: &str = "ai_embed_provider";
pub const KEY_EMBED_BASE_URL: &str = "ai_embed_base_url";
pub const KEY_EMBED_API_KEY: &str = "ai_embed_api_key";
/// 记录当前库存向量实际使用的模型名与维度（维度变更时触发清空重建）。
pub const KEY_EMBEDDED_MODEL: &str = "embedding_model";
pub const KEY_EMBEDDED_DIM: &str = "embedding_dim";
/// 语义向量自动回填开关（隐私门控，默认关闭）。
pub const KEY_SEMANTIC_BACKFILL: &str = "semantic_backfill_enabled";

/// 支持的 AI 服务提供方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProvider {
    /// 局域网 Ollama（`/api/chat` + `/api/embed`）。
    Ollama,
    /// OpenAI 兼容接口（`/v1/chat/completions` + `/v1/embeddings`）。
    OpenAi,
}

impl AiProvider {
    /// 从设置字符串解析；大小写不敏感，无法识别时回退 [`AiProvider::Ollama`]。
    pub fn parse(raw: &str) -> AiProvider {
        match raw.trim().to_lowercase().as_str() {
            "openai" | "openai_compatible" | "openai-compatible" => AiProvider::OpenAi,
            _ => AiProvider::Ollama,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AiProvider::Ollama => "ollama",
            AiProvider::OpenAi => "openai",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            // Android 模拟器访问宿主机；真机由用户改为局域网 IP。
            AiProvider::Ollama => "http://10.0.2.2:11434",
            AiProvider::OpenAi => "https://api.openai.com",
        }
    }

    pub fn default_chat_model(self) -> &'static str {
        match self {
            AiProvider::Ollama => "qwen3:4b",
            AiProvider::OpenAi => "gpt-4o-mini",
        }
    }

    pub fn default_embed_model(self) -> &'static str {
        match self {
            AiProvider::Ollama => "nomic-embed-text",
            AiProvider::OpenAi => "text-embedding-3-small",
        }
    }
}

/// AI 服务配置快照。
#[derive(Debug, Clone, PartialEq)]
pub struct AiConfig {
    pub provider: AiProvider,
    pub base_url: String,
    /// 仅 OpenAI 兼容接口使用；Ollama 留空。
    pub api_key: String,
    pub chat_model: String,
    pub embed_model: String,
    /// 向量服务独立配置；为空时回退对话服务对应字段。
    pub embed_provider: AiProvider,
    pub embed_base_url: String,
    pub embed_api_key: String,
}

impl AiConfig {
    /// 未填写服务地址视为未配置。
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty()
    }

    /// 去掉末尾 `/`，便于拼接路径。
    pub fn normalized_base_url(&self) -> String {
        self.base_url.trim().trim_end_matches('/').to_string()
    }

    /// 向量实际使用的 Provider（独立配置为空时回退对话 Provider）。
    pub fn effective_embed_provider(&self) -> AiProvider {
        if self.embed_base_url.trim().is_empty() {
            self.provider
        } else {
            self.embed_provider
        }
    }

    /// 向量实际使用的服务地址。
    pub fn effective_embed_base_url(&self) -> String {
        if self.embed_base_url.trim().is_empty() {
            self.base_url.clone()
        } else {
            self.embed_base_url.clone()
        }
    }

    /// 去掉末尾 `/` 的向量服务地址。
    pub fn normalized_embed_base_url(&self) -> String {
        self.effective_embed_base_url()
            .trim()
            .trim_end_matches('/')
            .to_string()
    }

    /// 向量实际使用的 API Key。
    pub fn effective_embed_api_key(&self) -> String {
        if self.embed_base_url.trim().is_empty() {
            self.api_key.clone()
        } else {
            self.embed_api_key.clone()
        }
    }

    /// 向量服务是否独立配置（embed_base_url 非空）。
    pub fn has_separate_embed_service(&self) -> bool {
        !self.embed_base_url.trim().is_empty()
    }
}

/// 某 Provider 的全默认配置。
pub fn default_config(provider: AiProvider) -> AiConfig {
    AiConfig {
        provider,
        base_url: provider.default_base_url().to_string(),
        api_key: String::new(),
        chat_model: provider.default_chat_model().to_string(),
        embed_model: provider.default_embed_model().to_string(),
        embed_provider: provider,
        embed_base_url: String::new(),
        embed_api_key: String::new(),
    }
}

/// 语义向量自动回填开关：默认关闭（避免物品文本未经显式开启就外发到 AI 服务）。
pub fn semantic_backfill_enabled(conn: &Connection) -> FinditResult<bool> {
    match get_setting(conn, KEY_SEMANTIC_BACKFILL)? {
        Some(v) => {
            let v = v.trim().to_ascii_lowercase();
            Ok(v == "1" || v == "true" || v == "yes" || v == "on")
        }
        None => Ok(false),
    }
}

/// 持久化语义向量自动回填开关（与 Dart 侧设置页共用同一键读写）。
pub fn set_semantic_backfill_enabled(conn: &Connection, enabled: bool) -> FinditResult<()> {
    set_setting(conn, KEY_SEMANTIC_BACKFILL, if enabled { "1" } else { "0" })
}

// ---------------------------------------------------------------------------
// 密钥落盘加密（S-H2）：保存时加密，读取时透明解密；空值保持空字符串。
// ---------------------------------------------------------------------------

fn resolve_keystore() -> FinditResult<Keystore> {
    // 测试注入优先（生产恒为 None），避免测试依赖全局 db_dir。
    if let Some(ks) = keystore::test_keystore() {
        return Ok(ks);
    }
    let dir = db::db_dir()?;
    keystore::load_or_create(&dir)
}

/// 落盘加密：空字符串不加密（保持空值，避免无意义密文）。
fn encrypt_key_value(plain: &str) -> FinditResult<String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    let ks = resolve_keystore()?;
    ks.encrypt_secret(plain)
}

/// 读取并透明解密；解密失败（如密钥文件缺失/轮换）按未设置处理，
/// 用户重新录入即可（数据库未初始化等场景原样返回）。
fn decrypt_key_value(stored: &str) -> String {
    match resolve_keystore() {
        Ok(ks) => ks.decrypt_secret(stored).unwrap_or_default(),
        Err(_) => stored.to_string(),
    }
}

/// 传输安全校验（S-M2）：配置了 API Key 时，服务地址必须为 https，
/// 禁止明文 http 传输密钥。无密钥的局域网 Ollama 等场景不受影响。
fn validate_transport(base_url: &str, api_key: &str) -> FinditResult<()> {
    if api_key.trim().is_empty() {
        return Ok(());
    }
    let url = base_url.trim().to_ascii_lowercase();
    if url.is_empty() {
        return Ok(()); // 空地址视为未配置，由调用方处理
    }
    if url.starts_with("http://") {
        return Err(FinditError::Validation(
            "安全限制：已配置 API Key 时服务地址必须使用 https（明文 http 会泄露密钥）"
                .to_string(),
        ));
    }
    Ok(())
}

/// 从 app_settings 读取配置；缺失项按当前 Provider 的默认值补齐。
pub fn load_ai_config(conn: &Connection) -> FinditResult<AiConfig> {
    let provider = match get_setting(conn, KEY_PROVIDER)? {
        Some(raw) => AiProvider::parse(&raw),
        None => AiProvider::Ollama,
    };
    let defaults = default_config(provider);
    let base_url = get_setting(conn, KEY_BASE_URL)?
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or(defaults.base_url);
    let api_key = get_setting(conn, KEY_API_KEY)?
        .map(|v| decrypt_key_value(&v))
        .unwrap_or_default();
    let chat_model = get_setting(conn, KEY_CHAT_MODEL)?
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or(defaults.chat_model);
    let embed_model = get_setting(conn, KEY_EMBED_MODEL)?
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or(defaults.embed_model);
    // 向量独立配置：缺失或为空时保持空字符串，运行时回退对话服务配置。
    let embed_provider = match get_setting(conn, KEY_EMBED_PROVIDER)? {
        Some(raw) if !raw.trim().is_empty() => AiProvider::parse(&raw),
        _ => provider,
    };
    let embed_base_url = get_setting(conn, KEY_EMBED_BASE_URL)?
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let embed_api_key = get_setting(conn, KEY_EMBED_API_KEY)?
        .map(|v| decrypt_key_value(&v))
        .unwrap_or_default();
    Ok(AiConfig {
        provider,
        base_url,
        api_key,
        chat_model,
        embed_model,
        embed_provider,
        embed_base_url,
        embed_api_key,
    })
}

/// 保存配置到 app_settings（整体覆盖）。密钥类字段落盘加密。
pub fn save_ai_config(conn: &Connection, config: &AiConfig) -> FinditResult<()> {
    // S-M2：带密钥的服务地址必须 https。
    validate_transport(&config.base_url, &config.api_key)?;
    if config.has_separate_embed_service() {
        validate_transport(&config.embed_base_url, &config.embed_api_key)?;
    }

    set_setting(conn, KEY_PROVIDER, config.provider.as_str())?;
    set_setting(conn, KEY_BASE_URL, config.base_url.trim())?;
    set_setting(conn, KEY_API_KEY, &encrypt_key_value(&config.api_key)?)?;
    set_setting(conn, KEY_CHAT_MODEL, config.chat_model.trim())?;
    set_setting(conn, KEY_EMBED_MODEL, config.embed_model.trim())?;
    set_setting(conn, KEY_EMBED_PROVIDER, config.embed_provider.as_str())?;
    set_setting(conn, KEY_EMBED_BASE_URL, config.embed_base_url.trim())?;
    set_setting(conn, KEY_EMBED_API_KEY, &encrypt_key_value(&config.embed_api_key)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;
    use uuid::Uuid;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    /// 准备带独立临时密钥库的测试环境（注入式，不依赖全局 db_dir / init_db，
    /// 避免与并行修复线在途的 FTS 迁移相互影响）。
    fn setup_with_keystore(tag: &str) -> Connection {
        let dir = std::env::temp_dir().join(format!(
            "findit-config-{tag}-{}",
            Uuid::new_v4().simple()
        ));
        let ks = crate::core::ai::keystore::load_or_create(&dir).unwrap();
        crate::core::ai::keystore::set_test_keystore(ks);
        setup()
    }

    #[test]
    fn provider_parse_is_lenient() {
        assert_eq!(AiProvider::parse("ollama"), AiProvider::Ollama);
        assert_eq!(AiProvider::parse("OpenAI"), AiProvider::OpenAi);
        assert_eq!(AiProvider::parse("  openai "), AiProvider::OpenAi);
        assert_eq!(AiProvider::parse("unknown"), AiProvider::Ollama);
        assert_eq!(AiProvider::parse(""), AiProvider::Ollama);
    }

    #[test]
    fn load_defaults_when_empty() {
        let conn = setup();
        let cfg = load_ai_config(&conn).unwrap();
        assert_eq!(cfg.provider, AiProvider::Ollama);
        assert_eq!(cfg.base_url, "http://10.0.2.2:11434");
        assert_eq!(cfg.chat_model, "qwen3:4b");
        assert_eq!(cfg.embed_model, "nomic-embed-text");
        assert!(cfg.api_key.is_empty());
        assert!(cfg.is_configured());
    }

    #[test]
    fn save_load_roundtrip() {
        let conn = setup_with_keystore("roundtrip");
        let cfg = AiConfig {
            provider: AiProvider::OpenAi,
            base_url: "https://my.llm.example/".to_string(),
            api_key: "sk-test".to_string(),
            chat_model: "qwen-plus".to_string(),
            embed_model: "text-embedding-v3".to_string(),
            embed_provider: AiProvider::Ollama,
            embed_base_url: "http://10.0.2.2:11434".to_string(),
            embed_api_key: String::new(),
        };
        save_ai_config(&conn, &cfg).unwrap();
        let loaded = load_ai_config(&conn).unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn missing_fields_fall_back_to_provider_defaults() {
        let conn = setup();
        set_setting(&conn, KEY_PROVIDER, "openai").unwrap();
        // 其余键缺失 → 用 OpenAI 默认值补齐。
        let cfg = load_ai_config(&conn).unwrap();
        assert_eq!(cfg.base_url, "https://api.openai.com");
        assert_eq!(cfg.chat_model, "gpt-4o-mini");
        assert_eq!(cfg.embed_model, "text-embedding-3-small");
    }

    #[test]
    fn blank_base_url_means_not_configured() {
        let conn = setup();
        set_setting(&conn, KEY_BASE_URL, "   ").unwrap();
        let cfg = load_ai_config(&conn).unwrap();
        // 空白按缺失处理，回退默认值（视为已配置）。
        assert!(cfg.is_configured());
        // 显式置空字符串同样回退默认值。
        let manual = AiConfig {
            base_url: "  ".to_string(),
            ..default_config(AiProvider::Ollama)
        };
        assert!(!manual.is_configured());
    }

    #[test]
    fn normalized_base_url_strips_trailing_slash() {
        let mut cfg = default_config(AiProvider::Ollama);
        cfg.base_url = "http://host:11434//".to_string();
        assert_eq!(cfg.normalized_base_url(), "http://host:11434");
    }

    #[test]
    fn embed_falls_back_to_chat_when_empty() {
        let cfg = default_config(AiProvider::OpenAi);
        // 独立向量为空 → 回退对话配置。
        assert_eq!(cfg.effective_embed_provider(), AiProvider::OpenAi);
        assert_eq!(cfg.effective_embed_base_url(), "https://api.openai.com");
        assert_eq!(cfg.effective_embed_api_key(), String::new());
        assert!(!cfg.has_separate_embed_service());
    }

    #[test]
    fn embed_uses_separate_config_when_set() {
        let mut cfg = default_config(AiProvider::OpenAi);
        cfg.embed_provider = AiProvider::Ollama;
        cfg.embed_base_url = "http://localhost:11434".to_string();
        cfg.embed_api_key = String::new();
        assert_eq!(cfg.effective_embed_provider(), AiProvider::Ollama);
        assert_eq!(cfg.effective_embed_base_url(), "http://localhost:11434");
        assert!(cfg.has_separate_embed_service());
    }

    #[test]
    fn separate_embed_service_roundtrip() {
        let conn = setup_with_keystore("embed-roundtrip");
        let cfg = AiConfig {
            provider: AiProvider::OpenAi,
            base_url: "https://api.openai.com".to_string(),
            api_key: "sk-chat".to_string(),
            chat_model: "gpt-4o-mini".to_string(),
            embed_model: "nomic-embed-text".to_string(),
            embed_provider: AiProvider::Ollama,
            embed_base_url: "http://10.0.2.2:11434".to_string(),
            embed_api_key: String::new(),
        };
        save_ai_config(&conn, &cfg).unwrap();
        let loaded = load_ai_config(&conn).unwrap();
        assert_eq!(loaded.embed_provider, AiProvider::Ollama);
        assert_eq!(loaded.embed_base_url, "http://10.0.2.2:11434");
        assert!(loaded.has_separate_embed_service());
    }

    #[test]
    fn saved_keys_are_encrypted_at_rest() {
        let conn = setup_with_keystore("at-rest");
        let cfg = AiConfig {
            provider: AiProvider::OpenAi,
            base_url: "https://api.openai.com".to_string(),
            api_key: "sk-chat-secret".to_string(),
            chat_model: "gpt-4o-mini".to_string(),
            embed_model: "nomic-embed-text".to_string(),
            embed_provider: AiProvider::OpenAi,
            embed_base_url: "https://api.openai.com".to_string(),
            embed_api_key: "sk-embed-secret".to_string(),
        };
        save_ai_config(&conn, &cfg).unwrap();
        // 落盘值必须是密文（enc:v1: 前缀），且不含明文。
        let chat_raw = get_setting(&conn, KEY_API_KEY).unwrap().unwrap();
        assert!(chat_raw.starts_with("enc:v1:"), "对话 Key 落盘应为密文");
        assert!(!chat_raw.contains("sk-chat-secret"), "密文不得含明文");
        let embed_raw = get_setting(&conn, KEY_EMBED_API_KEY).unwrap().unwrap();
        assert!(embed_raw.starts_with("enc:v1:"), "向量 Key 落盘应为密文");
        assert!(!embed_raw.contains("sk-embed-secret"), "密文不得含明文");
        // 读取透明解密。
        let loaded = load_ai_config(&conn).unwrap();
        assert_eq!(loaded.api_key, "sk-chat-secret");
        assert_eq!(loaded.embed_api_key, "sk-embed-secret");
    }

    #[test]
    fn http_with_key_is_rejected() {
        let conn = setup();
        // 对话服务：http + Key → 拒绝。
        let bad = AiConfig {
            provider: AiProvider::OpenAi,
            base_url: "http://192.168.1.10:8080/v1".to_string(),
            api_key: "sk-x".to_string(),
            chat_model: "m".to_string(),
            embed_model: "e".to_string(),
            embed_provider: AiProvider::OpenAi,
            embed_base_url: String::new(),
            embed_api_key: String::new(),
        };
        assert!(matches!(
            save_ai_config(&conn, &bad),
            Err(FinditError::Validation(_))
        ));
        // 向量独立服务：http + Key → 同样拒绝。
        let bad_embed = AiConfig {
            provider: AiProvider::Ollama,
            base_url: "http://localhost:11434".to_string(),
            api_key: String::new(),
            chat_model: "m".to_string(),
            embed_model: "e".to_string(),
            embed_provider: AiProvider::OpenAi,
            embed_base_url: "http://10.0.2.2:8080".to_string(),
            embed_api_key: "sk-embed".to_string(),
        };
        assert!(matches!(
            save_ai_config(&conn, &bad_embed),
            Err(FinditError::Validation(_))
        ));
        // 无密钥的 http 局域网服务不受影响。
        let ok = AiConfig {
            provider: AiProvider::Ollama,
            base_url: "http://192.168.1.10:11434".to_string(),
            api_key: String::new(),
            chat_model: "qwen3:4b".to_string(),
            embed_model: "nomic-embed-text".to_string(),
            embed_provider: AiProvider::Ollama,
            embed_base_url: String::new(),
            embed_api_key: String::new(),
        };
        save_ai_config(&conn, &ok).unwrap();
    }

    #[test]
    fn legacy_plaintext_key_still_loads() {
        let conn = setup_with_keystore("legacy");
        // 模拟旧版本直接写入的明文 Key：读取应原样透传（透明兼容）。
        set_setting(&conn, KEY_API_KEY, "sk-legacy-plain").unwrap();
        let cfg = load_ai_config(&conn).unwrap();
        assert_eq!(cfg.api_key, "sk-legacy-plain");
    }

    #[test]
    fn semantic_backfill_defaults_off_and_roundtrip() {
        let conn = setup();
        assert!(!semantic_backfill_enabled(&conn).unwrap(), "默认必须关闭");
        set_semantic_backfill_enabled(&conn, true).unwrap();
        assert!(semantic_backfill_enabled(&conn).unwrap());
        set_semantic_backfill_enabled(&conn, false).unwrap();
        assert!(!semantic_backfill_enabled(&conn).unwrap());
        // 宽松解析："1"/"true" 均视为开启。
        set_setting(&conn, KEY_SEMANTIC_BACKFILL, "1").unwrap();
        assert!(semantic_backfill_enabled(&conn).unwrap());
    }
}
