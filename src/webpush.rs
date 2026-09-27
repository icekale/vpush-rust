//! 浏览器 Web Push。只向已知推送服务发加密通知，不把订阅地址当普通 HTTP 调用。

use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use p256::ecdh::EphemeralSecret;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::{CryptoRng, RngCore};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use p256::PublicKey;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::db::Db;
use crate::feishu::Note;

const PRIV_KEY: &str = "vapid_private_key";
const PUB_KEY: &str = "vapid_public_key";
const DEFAULT_MAILTO: &str = "mailto:admin@localhost";
const HOSTS: &[&str] = &[
    "fcm.googleapis.com",
    "android.googleapis.com",
    "updates.push.services.mozilla.com",
    "web.push.apple.com",
];
const SUFFIXES: &[&str] = &[".notify.windows.com", ".push.apple.com"];

struct SysRng;
impl RngCore for SysRng {
    fn next_u32(&mut self) -> u32 {
        let mut buf = [0u8; 4];
        self.fill_bytes(&mut buf);
        u32::from_le_bytes(buf)
    }
    fn next_u64(&mut self) -> u64 {
        let mut buf = [0u8; 8];
        self.fill_bytes(&mut buf);
        u64::from_le_bytes(buf)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        getrandom::getrandom(dest).expect("系统随机数不可用");
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), p256::elliptic_curve::rand_core::Error> {
        getrandom::getrandom(dest).map_err(|_| p256::elliptic_curve::rand_core::Error::new("rng"))
    }
}
impl CryptoRng for SysRng {}

pub fn endpoint_ok(url: &str) -> bool {
    if url.is_empty() || url.len() > 2048 || !url.starts_with("https://") || url.contains('@') {
        return false;
    }
    let hostport = url[8..].split(['/', '?', '#']).next().unwrap_or("");
    let host = match hostport.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => hostport,
    };
    if host.is_empty() || host.starts_with('[') {
        return false;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    HOSTS.contains(&host.as_str()) || SUFFIXES.iter().any(|suffix| host.ends_with(suffix))
}

pub fn keys_ok(p256dh: &str, auth: &str) -> bool {
    match (b64url_decode(p256dh), b64url_decode(auth)) {
        (Ok(public), Ok(secret)) => public.len() == 65 && public[0] == 4 && secret.len() == 16,
        _ => false,
    }
}

pub async fn profile(db: &Db, user_id: i64) -> Result<(String, i64), String> {
    let keys = vapid_keys(db).await?;
    let count = db.webpush_count(user_id).await.map_err(|err| err.to_string())?;
    Ok((keys.public_b64, count))
}

pub async fn notify_note(db: &Db, user_id: i64, note: &Note<'_>, favorite: bool) -> Result<(), String> {
    let kind = if note.post_type == "reply" { " · 回复" } else { "" };
    let title = clip(&format!("{} · {}{kind}", note.kol_name, crate::push::platform_label(note.platform)), 60);
    let raw = if !note.content.trim().is_empty() {
        note.content
    } else if !note.title.trim().is_empty() {
        note.title
    } else {
        "（无正文）"
    };
    let mut body = raw.replace('\n', " ");
    if favorite {
        body = format!("★  {body}");
    }
    let payload = json!({
        "title": title,
        "body": clip(body.trim(), 180),
        "url": "/timeline",
        "tag": clip(&format!("post-{}", note.platform), 80),
    });
    send_payload(db, user_id, &payload).await
}

pub async fn send_text(db: &Db, user_id: i64, text: &str) -> Result<(), String> {
    let lines: Vec<&str> = text.trim().lines().collect();
    let title = clip(lines.first().copied().unwrap_or("V Push"), 60);
    let body = lines.get(1..).map(|rest| rest.join("\n")).unwrap_or_default();
    let body = if body.trim().is_empty() { title.clone() } else { body };
    let payload = json!({
        "title": title,
        "body": clip(body.trim(), 180),
        "url": click_url(text).unwrap_or_else(|| "/".into()),
        "tag": "text",
    });
    send_payload(db, user_id, &payload).await
}

async fn send_payload(db: &Db, user_id: i64, payload: &Value) -> Result<(), String> {
    let subs = db.webpush_subs(user_id).await.map_err(|err| err.to_string())?;
    if subs.is_empty() {
        return Err("用户未绑定浏览器通知".into());
    }
    let keys = vapid_keys(db).await?;
    let mailto = std::env::var("VAPID_MAILTO").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| DEFAULT_MAILTO.into());
    let body = serde_json::to_vec(payload).map_err(|err| err.to_string())?;
    let mut sent = 0;
    let mut last = String::new();
    for sub in subs {
        if !endpoint_ok(&sub.endpoint) || !keys_ok(&sub.p256dh, &sub.auth) {
            last = "推送订阅无效".into();
            continue;
        }
        let encrypted = match encrypt(&body, &sub.p256dh, &sub.auth) {
            Ok(bytes) => bytes,
            Err(err) => {
                last = err;
                continue;
            }
        };
        let auth = match vapid_header(&sub.endpoint, &keys.private_pem, &keys.public_b64, mailto.trim()) {
            Ok(value) => value,
            Err(err) => {
                last = err;
                continue;
            }
        };
        let endpoint = sub.endpoint.clone();
        let posted = tokio::task::spawn_blocking(move || post_encrypted(&endpoint, encrypted, auth))
            .await
            .map_err(|err| err.to_string())?;
        match posted {
            Ok(code) if (200..300).contains(&code) => sent += 1,
            Ok(404 | 410) => {
                let _ = db.delete_webpush_endpoint(&sub.endpoint).await;
            }
            Ok(code) => last = format!("HTTP {code}"),
            Err(err) => last = err,
        }
    }
    if sent == 0 {
        Err(if last.is_empty() { "浏览器推送订阅已全部失效".into() } else { last })
    } else {
        Ok(())
    }
}

struct Vapid {
    private_pem: String,
    public_b64: String,
}

async fn vapid_keys(db: &Db) -> Result<Vapid, String> {
    let env_priv = std::env::var("VAPID_PRIVATE_KEY").unwrap_or_default();
    let env_priv = normalize_pem(env_priv.trim());
    if !env_priv.is_empty() {
        let public = std::env::var("VAPID_PUBLIC_KEY").unwrap_or_default();
        let public = if public.trim().is_empty() { public_from_pem(&env_priv)? } else { public.trim().to_string() };
        return Ok(Vapid { private_pem: env_priv, public_b64: public });
    }
    let stored_priv = db.setting(PRIV_KEY).await.map_err(|err| err.to_string())?.unwrap_or_default();
    let stored_pub = db.setting(PUB_KEY).await.map_err(|err| err.to_string())?.unwrap_or_default();
    if !stored_priv.trim().is_empty() {
        let missing_pub = stored_pub.trim().is_empty();
        let public = if missing_pub { public_from_pem(stored_priv.trim())? } else { stored_pub };
        if missing_pub {
            db.set_setting(PUB_KEY, &public).await.map_err(|err| err.to_string())?;
        }
        return Ok(Vapid { private_pem: stored_priv, public_b64: public });
    }
    let pair = generate_vapid()?;
    db.set_setting(PRIV_KEY, &pair.private_pem).await.map_err(|err| err.to_string())?;
    db.set_setting(PUB_KEY, &pair.public_b64).await.map_err(|err| err.to_string())?;
    Ok(pair)
}

fn generate_vapid() -> Result<Vapid, String> {
    let signing = SigningKey::random(&mut SysRng);
    let private_pem = signing.to_pkcs8_pem(LineEnding::LF).map_err(|err| err.to_string())?.to_string();
    Ok(Vapid { public_b64: public_of(&signing), private_pem })
}

fn public_from_pem(pem: &str) -> Result<String, String> {
    let signing = SigningKey::from_pkcs8_pem(pem).map_err(|_| "VAPID 私钥无效".to_string())?;
    Ok(public_of(&signing))
}

fn public_of(signing: &SigningKey) -> String {
    b64url(signing.verifying_key().to_encoded_point(false).as_bytes())
}

fn encrypt(plaintext: &[u8], p256dh: &str, auth: &str) -> Result<Vec<u8>, String> {
    let ua_public = b64url_decode(p256dh).map_err(|_| "p256dh 无效")?;
    let auth_secret = b64url_decode(auth).map_err(|_| "auth 无效")?;
    let peer = PublicKey::from_sec1_bytes(&ua_public).map_err(|_| "p256dh 无效")?;
    let local = EphemeralSecret::random(&mut SysRng);
    let local_public = local.public_key().to_encoded_point(false);
    let shared = local.diffie_hellman(&peer);
    let salt: [u8; 16] = random_bytes();
    let info = concat_info(b"WebPush: info\0", &ua_public, local_public.as_bytes());
    let ikm = hkdf(&auth_secret, shared.raw_secret_bytes(), &info, 32)?;
    let cek = hkdf(&salt, &ikm, b"Content-Encoding: aes128gcm\0", 16)?;
    let nonce = hkdf(&salt, &ikm, b"Content-Encoding: nonce\0", 12)?;
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| "加密密钥无效")?;
    let mut record = plaintext.to_vec();
    record.push(2);
    let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), record.as_ref()).map_err(|_| "加密失败")?;
    let mut out = Vec::with_capacity(16 + 4 + 1 + 65 + ciphertext.len());
    out.extend(salt);
    out.extend(4096u32.to_be_bytes());
    out.push(65);
    out.extend(local_public.as_bytes());
    out.extend(ciphertext);
    Ok(out)
}

fn vapid_header(endpoint: &str, pem: &str, public_b64: &str, mailto: &str) -> Result<String, String> {
    let host = endpoint[8..].split(['/', '?', '#']).next().unwrap_or("");
    let header = b64url(br#"{"typ":"JWT","alg":"ES256"}"#);
    let exp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) + 12 * 3600;
    let claims = format!(r#"{{"aud":"https://{host}","exp":{exp},"sub":"{mailto}"}}"#);
    let input = format!("{header}.{}", b64url(claims.as_bytes()));
    let signing = SigningKey::from_pkcs8_pem(pem).map_err(|_| "VAPID 私钥无效")?;
    let sig: p256::ecdsa::Signature = signing.sign(input.as_bytes());
    Ok(format!("vapid t={input}.{}, k={public_b64}", b64url(&sig.to_bytes())))
}

fn post_encrypted(url: &str, body: Vec<u8>, auth: String) -> Result<u16, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .build();
    match agent
        .post(url)
        .set("TTL", "86400")
        .set("Urgency", "high")
        .set("Content-Encoding", "aes128gcm")
        .set("Content-Type", "application/octet-stream")
        .set("Authorization", &auth)
        .send_bytes(&body)
    {
        Ok(resp) => Ok(resp.status() as u16),
        Err(ureq::Error::Status(code, _)) => Ok(code as u16),
        Err(err) => Err(err.to_string()),
    }
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; len];
    Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, &mut out).map_err(|_| "密钥派生失败")?;
    Ok(out)
}

fn concat_info(prefix: &[u8], ua: &[u8], local: &[u8]) -> Vec<u8> {
    let mut info = Vec::with_capacity(prefix.len() + ua.len() + local.len());
    info.extend(prefix);
    info.extend(ua);
    info.extend(local);
    info
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).expect("系统随机数不可用");
    buf
}

fn b64url(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

fn b64url_decode(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD.decode(value.trim())
}

fn normalize_pem(raw: &str) -> String {
    if raw.contains('\n') { raw.to_string() } else { raw.replace("\\n", "\n") }
}

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

fn click_url(text: &str) -> Option<String> {
    let start = text.find("https://").or_else(|| text.find("http://"))?;
    let rest = &text[start..];
    let end = rest.find(|c: char| c.is_whitespace() || "。，、；：）)】」》\"'".contains(c)).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Payload;
    use p256::ecdh::diffie_hellman;
    use p256::SecretKey;

    fn decrypt(body: &[u8], ua: &SecretKey, auth: &[u8]) -> Vec<u8> {
        let salt = &body[..16];
        let idlen = body[20] as usize;
        let local = &body[21..21 + idlen];
        let ciphertext = &body[21 + idlen..];
        let peer = PublicKey::from_sec1_bytes(local).unwrap();
        let shared = diffie_hellman(ua.to_nonzero_scalar(), peer.as_affine());
        let ua_public = ua.public_key().to_encoded_point(false);
        let info = concat_info(b"WebPush: info\0", ua_public.as_bytes(), local);
        let ikm = hkdf(auth, shared.raw_secret_bytes(), &info, 32).unwrap();
        let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16).unwrap();
        let nonce = hkdf(salt, &ikm, b"Content-Encoding: nonce\0", 12).unwrap();
        let cipher = Aes128Gcm::new_from_slice(&cek).unwrap();
        let plain = cipher.decrypt(Nonce::from_slice(&nonce), Payload { msg: ciphertext, aad: b"" }).unwrap();
        assert_eq!(*plain.last().unwrap(), 2);
        plain[..plain.len() - 1].to_vec()
    }

    #[test]
    fn encryption_roundtrips_and_endpoints_are_restricted() {
        let ua = SecretKey::random(&mut SysRng);
        let public = b64url(ua.public_key().to_encoded_point(false).as_bytes());
        let auth = random_bytes::<16>();
        let auth_b64 = b64url(&auth);
        let message = br#"{"title":"hi"}"#;
        let sealed = encrypt(message, &public, &auth_b64).unwrap();
        assert_eq!(sealed[16..20], 4096u32.to_be_bytes());
        assert_eq!(sealed[20], 65);
        assert_eq!(decrypt(&sealed, &ua, &auth), message);
        let vapid = generate_vapid().unwrap();
        assert!(keys_ok(&vapid.public_b64, &auth_b64));
        let header = vapid_header("https://fcm.googleapis.com/fcm/send/abc", &vapid.private_pem, &vapid.public_b64, DEFAULT_MAILTO).unwrap();
        assert!(header.starts_with("vapid t=") && header.contains(&format!("k={}", vapid.public_b64)));
        assert_eq!(header.matches('.').count(), 2);
        assert!(endpoint_ok("https://fcm.googleapis.com/fcm/send/abc"));
        assert!(endpoint_ok("https://updates.push.services.mozilla.com/wpush/v2/abc"));
        assert!(endpoint_ok("https://web.push.apple.com/Q"));
        assert!(endpoint_ok("https://wns.example.notify.windows.com/a"));
        assert!(!endpoint_ok("http://fcm.googleapis.com/x"));
        assert!(!endpoint_ok("https://user:pass@fcm.googleapis.com/x"));
        assert!(!endpoint_ok("https://example.com/push"));
        assert!(!keys_ok("aaaa", &auth_b64));
    }
}
