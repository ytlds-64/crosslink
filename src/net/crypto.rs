use anyhow::{anyhow, Context, Result};
use aes_gcm::aead::KeyInit;
use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::Aead;
use hkdf::Hkdf;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use x25519_dalek::EphemeralSecret;
pub use x25519_dalek::{PublicKey, StaticSecret};

/// AES-256-GCM 会话加密器。
///
/// 发送方每次加密使用单调递增的 12 字节 nonce（前 4 字节为 0，后 8 字节为计数器）；
/// 接收方按帧携带的计数器解密，并拒绝重放 / 乱序（计数器必须严格递增）。
pub struct Crypter {
    cipher: Aes256Gcm,
    send_ctr: u64,
    last_recv: Option<u64>,
}

impl Crypter {
    pub fn new(key: &[u8; 32]) -> Self {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        Self {
            cipher,
            send_ctr: 0,
            last_recv: None,
        }
    }

    /// 加密明文，返回 (计数器, 密文+认证标签)
    pub fn encrypt(&mut self, plaintext: &[u8]) -> (u64, Vec<u8>) {
        let ctr = self.send_ctr;
        self.send_ctr += 1;
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&ctr.to_be_bytes());
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .expect("AES-GCM encryption must not fail with a valid key/nonce");
        (ctr, ct)
    }

    /// 解密（含重放保护）。`ctr` 由帧显式携带；首条消息直接接受，之后必须严格递增。
    pub fn decrypt(&mut self, ctr: u64, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if let Some(last) = self.last_recv {
            if ctr <= last {
                return Err(anyhow!(
                    "replay or out-of-order nonce: {} <= {}",
                    ctr,
                    last
                ));
            }
        }
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&ctr.to_be_bytes());
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext)
            .map_err(|e| anyhow!("AES-GCM decryption failed: {:?}", e))?;
        self.last_recv = Some(ctr);
        Ok(plaintext)
    }
}

/// 计算服务端静态公钥的指纹（SHA-256，冒号分隔的大写十六进制）。
/// 客户端据此对服务端身份做 pinning 信任。
pub fn fingerprint(pubkey: &[u8; 32]) -> String {
    let h = Sha256::digest(pubkey);
    h.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn public_key_and_fingerprint(key: &StaticSecret) -> (PublicKey, String) {
    let pk = PublicKey::from(key);
    (pk, fingerprint(pk.as_bytes()))
}

/// 载入或生成并持久化服务端静态身份（32 字节 X25519 私钥）。
/// 持久化后指纹跨运行保持稳定，便于客户端 pinning。
pub fn load_or_create_server_key(path: &Path) -> Result<StaticSecret> {
    if path.exists() {
        let bytes = std::fs::read(path).context("read server identity")?;
        if bytes.len() != 32 {
            return Err(anyhow!("corrupt server identity (expected 32 bytes)"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(StaticSecret::from(arr))
    } else {
        let secret = StaticSecret::random_from_rng(OsRng);
        std::fs::write(path, secret.to_bytes()).context("write server identity")?;
        log::info!("generated new server identity, saved to {}", path.display());
        Ok(secret)
    }
}

fn derive_key(shared: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(b"crosslink-v1"), shared);
    let mut okm = [0u8; 32];
    hk.expand(b"session-key", &mut okm)
        .expect("HKDF expand to 32 bytes is infallible");
    okm
}

/// 服务端握手：发送静态公钥 → 接收客户端临时公钥 → ECDH 派生会话密钥。
pub async fn server_handshake(
    stream: &mut tokio::net::TcpStream,
    key: &StaticSecret,
) -> Result<(Crypter, String)> {
    let server_pub = PublicKey::from(key);
    stream
        .write_all(server_pub.as_bytes())
        .await
        .context("send server public key")?;

    let mut client_pub_bytes = [0u8; 32];
    stream
        .read_exact(&mut client_pub_bytes)
        .await
        .context("recv client public key")?;
    let client_pub = PublicKey::from(client_pub_bytes);

    let shared = key.diffie_hellman(&client_pub);
    let session_key = derive_key(shared.as_bytes());
    let fp = fingerprint(server_pub.as_bytes());
    Ok((Crypter::new(&session_key), fp))
}

/// 客户端握手：接收服务端静态公钥（校验指纹）→ 发送临时公钥 → ECDH 派生会话密钥。
pub async fn client_handshake(
    stream: &mut tokio::net::TcpStream,
    expected: Option<&str>,
) -> Result<(Crypter, String)> {
    let mut server_pub_bytes = [0u8; 32];
    stream
        .read_exact(&mut server_pub_bytes)
        .await
        .context("recv server public key")?;
    let server_pub = PublicKey::from(server_pub_bytes);
    let fp = fingerprint(&server_pub_bytes);

    match expected {
        Some(exp) if !exp.eq_ignore_ascii_case(&fp) => {
            return Err(anyhow!(
                "server fingerprint mismatch!\n  expected: {}\n  received: {}",
                exp,
                fp
            ));
        }
        Some(_) => log::info!("server fingerprint verified: {}", fp),
        None => log::warn!("TOFU: no fingerprint provided, trusting server: {}", fp),
    }

    let client_secret = EphemeralSecret::random_from_rng(OsRng);
    let client_pub = PublicKey::from(&client_secret);
    stream
        .write_all(client_pub.as_bytes())
        .await
        .context("send client public key")?;

    let shared = client_secret.diffie_hellman(&server_pub);
    let session_key = derive_key(shared.as_bytes());
    Ok((Crypter::new(&session_key), fp))
}
