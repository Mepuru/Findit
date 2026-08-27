//! 密钥存储（过渡方案）：AI API Key 以 AES-256-GCM 加密后落盘。
//!
//! 加密主密钥 = SHA-256(随机机密 ‖ 随机盐)：随机机密与盐生成后写入应用
//! 私有数据目录下的 `.findit-keystore` 文件（Unix 权限 0600）。该文件
//! 不随备份 zip 导出，因此备份中的密钥密文无法被解密。
//!
//! 值格式：`enc:v1:{nonce_hex}:{ciphertext_hex}`（AES-256-GCM，随机 12 字节
//! nonce，密文后附 16 字节认证标签）；不带 `enc:` 前缀的值视为历史明文，
//! 读取时原样透传（兼容旧版本库，下次保存自动迁移为密文）。
//!
//! ⚠️ 过渡方案说明（安全评审 S-H2）：
//! 本实现是「Rust 核心无平台通道、禁止新增 Flutter 插件」约束下的落盘加密
//! 过渡方案。密钥材料位于应用沙盒内，对已取得文件系统访问的 root 级攻击者
//! 不具备强防护（与数据库同级保护）。正式迁移路径：
//! 1. 通过 platform channel / flutter_secure_storage 把主密钥放入
//!    Android Keystore / iOS Keychain；
//! 2. 本模块接口（`encrypt_secret` / `decrypt_secret`）保持不变，仅替换
//!    [`Keystore`] 的密钥来源（由 `load_or_create` 改为系统 Keystore 注入）；
//! 3. 现有密文可通过读取后重新加密一次性迁移。

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use std::path::Path;

use crate::core::error::{FinditError, FinditResult};

/// 加密值前缀；带此前缀的值按密文处理，否则视为历史明文。
pub const ENC_PREFIX: &str = "enc:v1:";

/// 密钥文件魔数（v1）。
const KEYSTORE_MAGIC: &[u8; 4] = b"FDK1";
/// 随机盐长度（保证每台设备/每次安装密钥唯一）。
const SALT_LEN: usize = 16;
/// 随机机密长度。
const SECRET_LEN: usize = 32;
/// 派生主密钥长度（AES-256）。
const KEY_LEN: usize = 32;
/// 密钥文件总长度：魔数 + 盐 + 机密。
const KEYFILE_LEN: usize = KEYSTORE_MAGIC.len() + SALT_LEN + SECRET_LEN;

/// 应用私有的落盘密钥库。
#[derive(Clone)]
pub struct Keystore {
    key: [u8; KEY_LEN],
}

impl Keystore {
    /// AES-256-GCM 加密，返回 `enc:v1:{nonce_hex}:{ciphertext_hex}`。
    pub fn encrypt_secret(&self, plaintext: &str) -> FinditResult<String> {
        let rng = SystemRandom::new();
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| FinditError::Io("安全随机数生成失败".to_string()))?;

        let unbound = UnboundKey::new(&AES_256_GCM, &self.key)
            .map_err(|_| FinditError::Io("AES-256-GCM 密钥初始化失败".to_string()))?;
        let key = LessSafeKey::new(unbound);

        let mut in_out = plaintext.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::empty(),
            &mut in_out,
        )
        .map_err(|_| FinditError::Io("密钥加密失败".to_string()))?;

        Ok(format!(
            "{ENC_PREFIX}{}:{}",
            to_hex(&nonce_bytes),
            to_hex(&in_out)
        ))
    }

    /// 解密。不带 `enc:` 前缀的值视为历史明文，原样返回（透明兼容旧库）。
    pub fn decrypt_secret(&self, blob: &str) -> FinditResult<String> {
        if !blob.starts_with(ENC_PREFIX) {
            return Ok(blob.to_string());
        }
        let rest = &blob[ENC_PREFIX.len()..];
        let (nonce_hex, ct_hex) = rest.split_once(':').ok_or_else(|| {
            FinditError::Validation("密钥密文格式非法（缺少分隔符）".to_string())
        })?;

        let nonce_bytes = from_hex(nonce_hex)
            .ok_or_else(|| FinditError::Validation("密钥密文 nonce 非法".to_string()))?;
        if nonce_bytes.len() != NONCE_LEN {
            return Err(FinditError::Validation("密钥密文 nonce 长度非法".to_string()));
        }
        let mut ct = from_hex(ct_hex)
            .ok_or_else(|| FinditError::Validation("密钥密文内容非法".to_string()))?;

        let unbound = UnboundKey::new(&AES_256_GCM, &self.key)
            .map_err(|_| FinditError::Io("AES-256-GCM 密钥初始化失败".to_string()))?;
        let key = LessSafeKey::new(unbound);
        let nonce_arr: [u8; NONCE_LEN] = nonce_bytes
            .try_into()
            .map_err(|_| FinditError::Validation("密钥密文 nonce 长度非法".to_string()))?;
        // open_in_place 返回的切片已去掉末尾 16 字节认证标签，
        // 不能直接用整个 Vec 做 UTF-8 转换。
        let opened = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_arr),
                Aad::empty(),
                &mut ct,
            )
            .map_err(|_| {
                FinditError::Validation("密钥密文无法解密（主密钥不匹配或已轮换）".to_string())
            })?;

        String::from_utf8(opened.to_vec())
            .map_err(|_| FinditError::Validation("解密结果不是合法 UTF-8".to_string()))
    }
}

/// 加载（或首次创建）应用私有密钥库。
///
/// 密钥文件位于 `db_dir/.findit-keystore`，不随备份导出。
pub fn load_or_create(db_dir: &Path) -> FinditResult<Keystore> {
    let path = db_dir.join(".findit-keystore");
    match std::fs::read(&path) {
        Ok(raw) => {
            if raw.len() != KEYFILE_LEN || &raw[..KEYSTORE_MAGIC.len()] != KEYSTORE_MAGIC {
                return Err(FinditError::Validation("密钥文件损坏或格式非法".to_string()));
            }
            let salt = &raw[KEYSTORE_MAGIC.len()..KEYSTORE_MAGIC.len() + SALT_LEN];
            let secret = &raw[KEYSTORE_MAGIC.len() + SALT_LEN..];
            Ok(Keystore {
                key: derive_key(salt, secret),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let rng = SystemRandom::new();
            let mut salt = [0u8; SALT_LEN];
            let mut secret = [0u8; SECRET_LEN];
            rng.fill(&mut salt)
                .map_err(|_| FinditError::Io("安全随机数生成失败".to_string()))?;
            rng.fill(&mut secret)
                .map_err(|_| FinditError::Io("安全随机数生成失败".to_string()))?;

            let key = derive_key(&salt, &secret);
            let mut raw = Vec::with_capacity(KEYFILE_LEN);
            raw.extend_from_slice(KEYSTORE_MAGIC);
            raw.extend_from_slice(&salt);
            raw.extend_from_slice(&secret);

            std::fs::create_dir_all(db_dir)?;
            write_private(&path, &raw)?;
            Ok(Keystore { key })
        }
        Err(e) => Err(e.into()),
    }
}

/// 主密钥派生：SHA-256(盐 ‖ 机密)。机密为 32 字节高熵随机数，盐保证
/// 每台设备/每次安装的密钥唯一；非口令场景下 SHA-256 即足够安全的 KDF。
fn derive_key(salt: &[u8], secret: &[u8]) -> [u8; KEY_LEN] {
    let mut material = Vec::with_capacity(salt.len() + secret.len());
    material.extend_from_slice(salt);
    material.extend_from_slice(secret);
    let d = digest(&SHA256, &material);
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&d.as_ref()[..KEY_LEN]);
    key
}

#[cfg(unix)]
fn write_private(path: &Path, raw: &[u8]) -> FinditResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, raw)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, raw: &[u8]) -> FinditResult<()> {
    std::fs::write(path, raw)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试注入钩子：config 测试在不初始化全局 db_dir 时也能解析密钥库。
// 生产构建恒为 None，走 `db::db_dir()` 真实路径。
// ---------------------------------------------------------------------------

#[cfg(test)]
static TEST_KEYSTORE: std::sync::Mutex<Option<Keystore>> = std::sync::Mutex::new(None);

/// 返回测试注入的密钥库（生产构建恒为 `None`）。
pub fn test_keystore() -> Option<Keystore> {
    #[cfg(test)]
    {
        TEST_KEYSTORE.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    #[cfg(not(test))]
    {
        None
    }
}

/// 注入临时密钥库（仅供测试；覆盖后由下一次注入或进程结束清理）。
#[cfg(test)]
pub fn set_test_keystore(ks: Keystore) {
    *TEST_KEYSTORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(ks);
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "findit-keystore-{tag}-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let dir = temp_dir("roundtrip");
        let ks = load_or_create(&dir).unwrap();
        let plain = "sk-test-中文密钥-12345";
        let blob = ks.encrypt_secret(plain).unwrap();
        assert!(blob.starts_with(ENC_PREFIX));
        assert!(!blob.contains(plain), "密文不得包含明文");
        assert_eq!(ks.decrypt_secret(&blob).unwrap(), plain);
    }

    #[test]
    fn keyfile_is_reused_across_loads() {
        let dir = temp_dir("reuse");
        let ks1 = load_or_create(&dir).unwrap();
        let blob = ks1.encrypt_secret("sk-1").unwrap();
        // 第二次加载应读取同一密钥文件，能解密第一次的密文。
        let ks2 = load_or_create(&dir).unwrap();
        assert_eq!(ks2.decrypt_secret(&blob).unwrap(), "sk-1");
        // 密钥文件存在且长度正确。
        let raw = std::fs::read(dir.join(".findit-keystore")).unwrap();
        assert_eq!(raw.len(), KEYFILE_LEN);
        assert_eq!(&raw[..4], KEYSTORE_MAGIC);
    }

    #[test]
    fn different_installs_use_different_keys() {
        let dir_a = temp_dir("inst-a");
        let dir_b = temp_dir("inst-b");
        let ks_a = load_or_create(&dir_a).unwrap();
        let ks_b = load_or_create(&dir_b).unwrap();
        let blob = ks_a.encrypt_secret("sk-x").unwrap();
        // 另一安装的密钥无法解密本安装的密文。
        assert!(matches!(
            ks_b.decrypt_secret(&blob),
            Err(FinditError::Validation(_))
        ));
    }

    #[test]
    fn legacy_plaintext_passes_through() {
        let dir = temp_dir("legacy");
        let ks = load_or_create(&dir).unwrap();
        assert_eq!(ks.decrypt_secret("sk-plain-old").unwrap(), "sk-plain-old");
        assert_eq!(ks.decrypt_secret("").unwrap(), "");
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let dir = temp_dir("tamper");
        let ks = load_or_create(&dir).unwrap();
        let blob = ks.encrypt_secret("sk-secret").unwrap();
        // 篡改一个字符（GCM 认证标签应拦截）。
        let mut chars: Vec<char> = blob.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == '0' { '1' } else { '0' };
        let tampered: String = chars.into_iter().collect();
        assert!(matches!(
            ks.decrypt_secret(&tampered),
            Err(FinditError::Validation(_))
        ));
    }

    #[test]
    fn empty_value_roundtrip() {
        let dir = temp_dir("empty");
        let ks = load_or_create(&dir).unwrap();
        assert_eq!(ks.decrypt_secret(&ks.encrypt_secret("").unwrap()).unwrap(), "");
    }
}
