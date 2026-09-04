//! 凭据静态加密（封印）。
//!
//! 库里存的 LLM API key、问数连接串、来源配置里的 token、api 来源的推送密钥，从前都是
//! 明文——能读到库（或一份备份）的人就拿到了全部外部凭据。现在这些值在落库前用
//! AES-256-GCM 封印，密钥不在库里：环境变量 `UTOPIA_SECRET_KEY`，或数据目录下
//! 首次启动生成的 `secret.key`。威胁模型是「库泄漏 ≠ 凭据泄漏」：一份 pg_dump、一个
//! 只读库账号，拿到的是密文。能同时读到数据目录和库的人（登上了服务器）不在此列——
//! 那是服务器访问控制的事。
//!
//! **格式**：`enc:v1:` + base64(nonce(12) ‖ 密文+tag)。没有前缀的值按明文读（升级前
//! 落库的旧行），启动时 [`crate::secrets`] 的调用方（store 的 backfill）把它们补封。
//! 封印幂等：已封印的值再封一次原样返回，读路径与写路径都不必先判断。
//!
//! **一个进程一把钥匙**：服务启动时 [`init`] 一次；没初始化时 [`seal`] 原样返回、
//! [`open`] 只认明文——单元测试与没有服务的工具走这条。
//!
//! 密钥轮换不在这一版：换钥匙 = 用旧钥匙读出、新钥匙写回，是一次显式的迁移。

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::sync::OnceLock;

pub const PREFIX: &str = "enc:v1:";
const NONCE_LEN: usize = 12;

static SEALER: OnceLock<Aes256Gcm> = OnceLock::new();

/// 装钥匙。只认第一次；再调返回 false，钥匙不变
pub fn init(key: [u8; 32]) -> bool {
    SEALER
        .set(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)))
        .is_ok()
}

pub fn is_ready() -> bool {
    SEALER.get().is_some()
}

/// 生一把新钥匙（CSPRNG 32 字节）
pub fn generate_key() -> [u8; 32] {
    Aes256Gcm::generate_key(OsRng).into()
}

/// 钥匙的文本形态：64 位十六进制或 44 位 base64，都收
pub fn parse_key(text: &str) -> Option<[u8; 32]> {
    let t = text.trim();
    let bytes: Vec<u8> = if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..64)
            .step_by(2)
            .map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()?
    } else {
        B64.decode(t).ok()?
    };
    bytes.try_into().ok()
}

pub fn key_to_hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn is_sealed(stored: &str) -> bool {
    stored.starts_with(PREFIX)
}

/// 封印。幂等：已封印的原样返回；没装钥匙时原样返回（明文落库，和从前一样）
pub fn seal(plain: &str) -> String {
    if is_sealed(plain) {
        return plain.to_string();
    }
    let Some(cipher) = SEALER.get() else {
        return plain.to_string();
    };
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let mut out = cipher
        .encrypt(&nonce, plain.as_bytes())
        .expect("AES-GCM encryption does not fail on in-memory input");
    let mut bytes = nonce.to_vec();
    bytes.append(&mut out);
    format!("{PREFIX}{}", B64.encode(bytes))
}

/// 开封。没有前缀的按明文原样返回（升级前的旧行）；有前缀而钥匙不对或被改过，报错——
/// 静默回一段乱码会让下游拿它去调外部接口，错得更远
pub fn open(stored: &str) -> anyhow::Result<String> {
    let Some(body) = stored.strip_prefix(PREFIX) else {
        return Ok(stored.to_string());
    };
    let Some(cipher) = SEALER.get() else {
        anyhow::bail!("a sealed secret was read before the secret key was loaded");
    };
    let bytes = B64
        .decode(body)
        .map_err(|_| anyhow::anyhow!("a sealed secret is not valid base64"))?;
    if bytes.len() < NONCE_LEN {
        anyhow::bail!("a sealed secret is too short");
    }
    let (nonce, ct) = bytes.split_at(NONCE_LEN);
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| anyhow::anyhow!("a sealed secret does not open with this key (wrong UTOPIA_SECRET_KEY, or the value was tampered with)"))?;
    String::from_utf8(plain).map_err(|_| anyhow::anyhow!("a sealed secret is not UTF-8"))
}

pub fn seal_opt(plain: Option<&str>) -> Option<String> {
    plain.map(seal)
}

pub fn open_opt(stored: Option<&str>) -> anyhow::Result<Option<String>> {
    stored.map(open).transpose()
}

/// 把 JSON 对象里给定键的字符串值封印（就地）。非字符串值不动
pub fn seal_json_keys(value: &mut serde_json::Value, keys: &[&str]) {
    if let Some(obj) = value.as_object_mut() {
        for key in keys {
            if let Some(serde_json::Value::String(s)) = obj.get(*key) {
                let sealed = seal(s);
                obj.insert((*key).to_string(), serde_json::Value::String(sealed));
            }
        }
    }
}

/// [`seal_json_keys`] 的反向
pub fn open_json_keys(value: &mut serde_json::Value, keys: &[&str]) -> anyhow::Result<()> {
    if let Some(obj) = value.as_object_mut() {
        for key in keys {
            if let Some(serde_json::Value::String(s)) = obj.get(*key) {
                let opened = open(s)?;
                obj.insert((*key).to_string(), serde_json::Value::String(opened));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() {
        // 测试二进制里只装一次；后面的测试拿同一把
        let _ = init([7u8; 32]);
    }

    #[test]
    fn a_secret_round_trips_and_never_repeats() {
        ready();
        let a = seal("sk-live-123");
        let b = seal("sk-live-123");
        assert!(is_sealed(&a));
        assert_ne!(a, b, "a fresh nonce every time");
        assert_eq!(open(&a).unwrap(), "sk-live-123");
        assert_eq!(open(&b).unwrap(), "sk-live-123");
    }

    #[test]
    fn sealing_is_idempotent_and_plaintext_passes_through() {
        ready();
        let once = seal("token");
        assert_eq!(seal(&once), once, "already sealed stays as it is");
        assert_eq!(open("legacy-plain").unwrap(), "legacy-plain");
    }

    #[test]
    fn a_tampered_value_does_not_open() {
        ready();
        let sealed = seal("secret");
        let mut broken = sealed.clone();
        broken.pop();
        broken.push(if sealed.ends_with('A') { 'B' } else { 'A' });
        assert!(open(&broken).is_err());
        assert!(open("enc:v1:not-base64!").is_err());
    }

    #[test]
    fn json_keys_are_sealed_in_place() {
        ready();
        let mut cfg = serde_json::json!({ "token": "t0k", "url": "https://x", "n": 3 });
        seal_json_keys(&mut cfg, &["token", "url_missing", "n"]);
        assert!(is_sealed(cfg["token"].as_str().unwrap()));
        assert_eq!(cfg["url"], "https://x");
        assert_eq!(cfg["n"], 3);
        open_json_keys(&mut cfg, &["token"]).unwrap();
        assert_eq!(cfg["token"], "t0k");
    }

    #[test]
    fn keys_parse_from_hex_and_base64() {
        let key = generate_key();
        assert_eq!(parse_key(&key_to_hex(&key)), Some(key));
        assert_eq!(parse_key(&B64.encode(key)), Some(key));
        assert_eq!(parse_key("too-short"), None);
    }
}
