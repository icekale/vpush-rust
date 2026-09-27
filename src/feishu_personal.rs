use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
#[cfg(test)]
use tokio::sync::OnceCell;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use serde_json::{json, Value};

use crate::db::{Db, FeishuBot, FeishuSession};

const BASE: &str = "https://accounts.feishu.cn";
const LARK: &str = "https://accounts.larksuite.com";
const BIND_TTL: i64 = 600;
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: OnceCell<tokio::sync::Mutex<()>> = OnceCell::const_new();

static CODES: Mutex<Option<HashMap<String, (String, i64)>>> = Mutex::new(None);

pub fn enabled() -> bool {
    credential_key().is_some()
}

pub fn mask_app_id(app_id: &str) -> String {
    if app_id.len() > 8 {
        format!("{}…", &app_id[..8])
    } else {
        "…".into()
    }
}

pub async fn begin(db: &Db, user_id: i64, now: i64) -> Result<Value, PersonalError> {
    let key = credential_key().ok_or(PersonalError::Disabled)?;
    db.cancel_feishu_sessions(user_id).await?;
    let (body, base) = tokio::task::spawn_blocking(|| post_begin(BASE))
        .await
        .map_err(|_| PersonalError::Upstream("发起注册失败".into()))?
        .map_err(|err| PersonalError::Upstream(format!("发起注册失败：{err}")))?;
    let started = parse_begin(&body)?;
    let session_id = session_token()?;
    let cipher = seal(&key, &started.device_code)
        .map_err(|err| PersonalError::Upstream(format!("发起注册失败：{err}")))?;
    let expires_at = now + started.expires_in;
    db.create_feishu_session(
        &session_id,
        user_id,
        &cipher,
        &base,
        &started.verification_uri,
        expires_at,
        started.interval,
    )
    .await?;
    spawn_poller(db.clone(), session_id.clone());
    let session = db
        .feishu_session(&session_id)
        .await?
        .ok_or(PersonalError::Missing)?;
    Ok(payload(
        &session,
        db.feishu_bot(user_id).await?.as_ref(),
        now,
    ))
}

pub async fn status(
    db: &Db,
    user_id: i64,
    session_id: &str,
    now: i64,
) -> Result<Value, PersonalError> {
    let session = owned(db, user_id, session_id).await?;
    Ok(payload(
        &session,
        db.feishu_bot(user_id).await?.as_ref(),
        now,
    ))
}

pub async fn refresh_code(
    db: &Db,
    user_id: i64,
    session_id: &str,
    now: i64,
) -> Result<Value, PersonalError> {
    let session = owned(db, user_id, session_id).await?;
    if session.status != "awaiting_bind" {
        return Err(PersonalError::Bad("当前状态无需刷新绑定码"));
    }
    issue_code(db, &session, now).await?;
    let session = owned(db, user_id, session_id).await?;
    Ok(payload(
        &session,
        db.feishu_bot(user_id).await?.as_ref(),
        now,
    ))
}

pub async fn cancel(db: &Db, user_id: i64, session_id: &str) -> Result<(), PersonalError> {
    let session = owned(db, user_id, session_id).await?;
    if !matches!(session.status.as_str(), "expired" | "cancelled" | "active") {
        db.set_feishu_status(session_id, "cancelled", "").await?;
    }
    drop_code(session_id);
    Ok(())
}

pub async fn disable(db: &Db, user_id: i64) -> Result<(), PersonalError> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT session_id FROM feishu_registration_sessions WHERE user_id = ? AND status NOT IN ('expired', 'cancelled')",
    )
    .bind(user_id)
    .fetch_all(db.pool())
    .await?;
    db.delete_feishu_bot(user_id).await?;
    db.cancel_feishu_sessions(user_id).await?;
    for session_id in rows {
        drop_code(&session_id);
    }
    Ok(())
}

pub fn parse_bind_code(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix("/bind")?.trim();
    if rest.len() == 6 && rest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(rest)
    } else {
        None
    }
}

pub async fn accept_bind<F>(
    db: &Db,
    session_id: &str,
    code: &str,
    sender_open_id: &str,
    chat_id: &str,
    now: i64,
    send_test: F,
) -> Result<bool, PersonalError>
where
    F: FnOnce(&str, &str, &str, &str) -> Result<(), String>,
{
    let Some(session) = db.feishu_session(session_id).await? else {
        return Ok(false);
    };
    if session.status != "awaiting_bind" || chat_id.trim().is_empty() {
        return Ok(false);
    }
    if session.bind_code_hash.is_empty() || session.bind_code_expires_at.unwrap_or(0) < now {
        db.set_feishu_status(session_id, "degraded", "绑定码已过期")
            .await?;
        drop_code(session_id);
        return Ok(false);
    }
    if hash_code(code) != session.bind_code_hash {
        return Ok(false);
    }
    if !session.expected_open_id.is_empty() && sender_open_id != session.expected_open_id {
        return Ok(false);
    }
    db.clear_feishu_bind_code(session_id, "testing").await?;
    drop_code(session_id);
    let Some(key) = credential_key() else {
        db.set_feishu_status(session_id, "degraded", "服务端未配置 FEISHU_CREDENTIAL_KEY")
            .await?;
        return Err(PersonalError::Disabled);
    };
    let secret = match open_secret(&key, &session.candidate_app_secret_ciphertext) {
        Ok(secret) => secret,
        Err(_) => {
            db.set_feishu_status(session_id, "degraded", "候选凭据解密失败")
                .await?;
            return Ok(false);
        }
    };
    let host = if session.candidate_tenant_brand == "lark" {
        "open.larksuite.com"
    } else {
        "open.feishu.cn"
    };
    if let Err(err) = send_test(&session.candidate_app_id, &secret, chat_id, host) {
        db.set_feishu_status(session_id, "degraded", &clip(&err))
            .await?;
        return Ok(false);
    }
    if let Err(err) = db
        .save_feishu_bot(
            session.user_id,
            &session.candidate_app_id,
            &session.candidate_app_secret_ciphertext,
            &session.candidate_tenant_brand,
            sender_open_id,
            chat_id,
        )
        .await
    {
        let message = err.to_string();
        db.set_feishu_status(session_id, "degraded", &clip(&message))
            .await?;
        return Ok(false);
    }
    db.set_feishu_status(session_id, "active", "").await?;
    Ok(true)
}

pub async fn apply_poll(
    db: &Db,
    session_id: &str,
    body: &Value,
    now: i64,
) -> Result<bool, PersonalError> {
    let Some(session) = db.feishu_session(session_id).await? else {
        return Ok(true);
    };
    if session.status != "pending" {
        return Ok(true);
    }
    if session.session_expires_at < now {
        db.set_feishu_status(session_id, "expired", "").await?;
        return Ok(true);
    }
    match interpret(body) {
        Poll::Pending => Ok(false),
        Poll::SlowDown => Ok(false),
        Poll::Denied => {
            db.set_feishu_status(session_id, "cancelled", "access_denied")
                .await?;
            Ok(true)
        }
        Poll::Expired(code) => {
            db.set_feishu_status(session_id, "expired", &code).await?;
            Ok(true)
        }
        Poll::Degraded(message) => {
            db.set_feishu_status(session_id, "degraded", &clip(&message))
                .await?;
            Ok(true)
        }
        Poll::Ready {
            client_id,
            client_secret,
            open_id,
            lark,
        } => {
            let key = credential_key().ok_or(PersonalError::Disabled)?;
            let secret =
                seal(&key, &client_secret).map_err(|_| PersonalError::Bad("候选凭据加密失败"))?;
            let mut base = session.registration_base_url;
            if lark {
                base = LARK.into();
            }
            db.save_feishu_credentials(
                session_id,
                &client_id,
                &secret,
                if lark { "lark" } else { "feishu" },
                &open_id,
                &base,
            )
            .await?;
            let session = db
                .feishu_session(session_id)
                .await?
                .ok_or(PersonalError::Missing)?;
            issue_code(db, &session, now).await?;
            Ok(true)
        }
    }
}

pub fn payload(session: &FeishuSession, bot: Option<&FeishuBot>, now: i64) -> Value {
    let (command, expires) = if session.status == "awaiting_bind" {
        remembered(&session.session_id, now)
            .map(|(code, expires)| (format!("/bind {code}"), Some(expires)))
            .unwrap_or((String::new(), session.bind_code_expires_at))
    } else {
        (String::new(), session.bind_code_expires_at)
    };
    json!({
        "session_id": session.session_id,
        "status": session.status,
        "verification_uri": session.verification_uri,
        "qr_uri": "",
        "session_expires_at": session.session_expires_at,
        "bind_command": command,
        "bind_code_expires_at": expires,
        "last_error": session.last_error,
        "candidate_app_id_masked": if session.candidate_app_id.is_empty() { String::new() } else { mask_app_id(&session.candidate_app_id) },
        "personal_bot_status": bot.map(|item| item.status.as_str()).unwrap_or(""),
        "personal_bot_app_id_masked": bot.map(|item| mask_app_id(&item.app_id)).unwrap_or_default(),
    })
}

fn spawn_poller(db: Db, session_id: String) {
    tokio::spawn(async move {
        let mut interval;
        loop {
            let now = now_secs();
            let Ok(Some(session)) = db.feishu_session(&session_id).await else {
                return;
            };
            if session.status != "pending" {
                return;
            }
            interval = session.poll_interval.max(1) as u64;
            if session.session_expires_at < now {
                let _ = db.set_feishu_status(&session_id, "expired", "").await;
                return;
            }
            let Some(key) = credential_key() else { return };
            let Ok(code) = open_secret(&key, &session.device_code_ciphertext) else {
                let _ = db
                    .set_feishu_status(&session_id, "degraded", "设备码解密失败")
                    .await;
                return;
            };
            let base = session.registration_base_url.clone();
            let polled = tokio::task::spawn_blocking(move || post_poll(&base, &code)).await;
            let stop = match polled {
                Ok(Ok(body)) => apply_poll(&db, &session_id, &body, now)
                    .await
                    .unwrap_or(true),
                Ok(Err(Flow::Pending)) => false,
                Ok(Err(Flow::SlowDown)) => {
                    interval = (interval + 5).min(30);
                    false
                }
                Ok(Err(Flow::Stop(status, message))) => {
                    let _ = db.set_feishu_status(&session_id, status, &message).await;
                    true
                }
                Err(_) | Ok(Err(Flow::Retry)) => false,
            };
            if stop {
                return;
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
    });
}

#[derive(Debug)]
pub enum PersonalError {
    Disabled,
    Missing,
    Bad(&'static str),
    Upstream(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for PersonalError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err)
    }
}

struct Started {
    device_code: String,
    verification_uri: String,
    expires_in: i64,
    interval: i64,
}

fn parse_begin(body: &Value) -> Result<Started, PersonalError> {
    let device_code = text(body, "device_code");
    let verification_uri = {
        let complete = text(body, "verification_uri_complete");
        if complete.is_empty() {
            text(body, "verification_uri")
        } else {
            complete
        }
    };
    if device_code.is_empty() || verification_uri.is_empty() {
        return Err(PersonalError::Upstream(format!(
            "发起注册失败：{}",
            text(body, "error")
        )));
    }
    Ok(Started {
        device_code,
        verification_uri,
        expires_in: body
            .get("expires_in")
            .and_then(Value::as_i64)
            .unwrap_or(3600)
            .max(1),
        interval: body
            .get("interval")
            .and_then(Value::as_i64)
            .unwrap_or(5)
            .clamp(1, 30),
    })
}

enum Poll {
    Pending,
    SlowDown,
    Denied,
    Expired(String),
    Degraded(String),
    Ready {
        client_id: String,
        client_secret: String,
        open_id: String,
        lark: bool,
    },
}

fn interpret(body: &Value) -> Poll {
    let error = text(body, "error");
    if error == "authorization_pending" {
        return Poll::Pending;
    }
    if error == "slow_down" {
        return Poll::SlowDown;
    }
    if error == "access_denied" {
        return Poll::Denied;
    }
    if error == "expired_token" || error == "invalid_grant" {
        return Poll::Expired(error);
    }
    if !error.is_empty() {
        return Poll::Degraded(text(body, "error_description"));
    }
    let client_id = first(body, &["client_id", "app_id"]);
    let client_secret = first(body, &["client_secret", "app_secret"]);
    let info = body.get("user_info").cloned().unwrap_or(Value::Null);
    let open_id = text(&info, "open_id");
    if client_id.is_empty() || client_secret.is_empty() || open_id.is_empty() {
        return Poll::Degraded("注册结果缺少 client_id/client_secret/open_id".into());
    }
    Poll::Ready {
        client_id,
        client_secret,
        open_id,
        lark: text(&info, "tenant_brand").eq_ignore_ascii_case("lark"),
    }
}

enum Flow {
    Pending,
    SlowDown,
    Retry,
    Stop(&'static str, String),
}

fn post_begin(base: &str) -> Result<(Value, String), String> {
    let body = post_form(
        &format!("{base}/oauth/v1/app/registration"),
        &[
            ("action", "begin"),
            ("archetype", "PersonalAgent"),
            ("auth_method", "client_secret"),
            ("request_user_info", "open_id"),
        ],
    )?;
    if text(&body, "device_code").is_empty() || text(&body, "verification_uri").is_empty() {
        return Err(text(&body, "error"));
    }
    Ok((body, base.into()))
}

fn post_poll(base: &str, device_code: &str) -> Result<Value, Flow> {
    let body = post_form(
        &format!("{base}/oauth/v1/app/registration"),
        &[("action", "poll"), ("device_code", device_code)],
    )
    .map_err(|_| Flow::Retry)?;
    let error = text(&body, "error");
    if error == "authorization_pending" {
        return Err(Flow::Pending);
    }
    if error == "slow_down" {
        return Err(Flow::SlowDown);
    }
    if error == "access_denied" {
        return Err(Flow::Stop("cancelled", "access_denied".into()));
    }
    if error == "expired_token" || error == "invalid_grant" {
        return Err(Flow::Stop("expired", error));
    }
    if !error.is_empty() {
        return Err(Flow::Stop(
            "degraded",
            clip(&text(&body, "error_description")),
        ));
    }
    Ok(body)
}

fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build();
    let response = match agent.post(url).send_form(fields) {
        Ok(response) => response,
        Err(ureq::Error::Status(400, response)) => response,
        Err(err) => return Err(err.to_string()),
    };
    let text = response.into_string().map_err(|err| err.to_string())?;
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

async fn issue_code(db: &Db, session: &FeishuSession, now: i64) -> Result<(), PersonalError> {
    if session.session_expires_at < now {
        db.set_feishu_status(&session.session_id, "expired", "")
            .await?;
        return Err(PersonalError::Bad("注册会话已过期，请重新扫码"));
    }
    let code = hex::encode(random_bytes(3)?);
    let expires = now + BIND_TTL;
    db.save_feishu_bind_code(&session.session_id, &hash_code(&code), expires)
        .await?;
    CODES
        .lock()
        .expect("feishu codes")
        .get_or_insert_with(HashMap::new)
        .insert(session.session_id.clone(), (code, expires));
    crate::feishu_ws::ensure_listener(db.clone(), session.session_id.clone());
    Ok(())
}

async fn owned(db: &Db, user_id: i64, session_id: &str) -> Result<FeishuSession, PersonalError> {
    match db.feishu_session(session_id).await? {
        Some(session) if session.user_id == user_id => Ok(session),
        _ => Err(PersonalError::Missing),
    }
}

fn remembered(session_id: &str, now: i64) -> Option<(String, i64)> {
    let mut guard = CODES.lock().expect("feishu codes");
    let map = guard.get_or_insert_with(HashMap::new);
    match map.get(session_id).cloned() {
        Some((code, expires)) if expires >= now => Some((code, expires)),
        Some(_) => {
            map.remove(session_id);
            None
        }
        None => None,
    }
}

fn drop_code(session_id: &str) {
    if let Some(map) = CODES.lock().expect("feishu codes").as_mut() {
        map.remove(session_id);
    }
}

pub(crate) fn credential_key() -> Option<String> {
    let key = std::env::var("FEISHU_CREDENTIAL_KEY").unwrap_or_default();
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

pub(crate) fn seal(key_b64: &str, plain: &str) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(&decode_key(key_b64)?)
        .map_err(|_| "FEISHU_CREDENTIAL_KEY 无效".to_string())?;
    let nonce_bytes = random_bytes(12).map_err(|_| "无法生成密钥".to_string())?;
    let mut packed = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_bytes())
        .map_err(|_| "无法加密凭据".to_string())?;
    let mut out = nonce_bytes;
    out.append(&mut packed);
    Ok(URL_SAFE.encode(out))
}

pub(crate) fn open_app_secret(key_b64: &str, stored: &str) -> Result<String, String> {
    open_secret(key_b64, stored)
}

fn open_secret(key_b64: &str, stored: &str) -> Result<String, String> {
    let packed = URL_SAFE
        .decode(stored)
        .map_err(|_| "密文无效".to_string())?;
    if packed.len() < 13 {
        return Err("密文无效".into());
    }
    let cipher = Aes256Gcm::new_from_slice(&decode_key(key_b64)?)
        .map_err(|_| "FEISHU_CREDENTIAL_KEY 无效".to_string())?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&packed[..12]), &packed[12..])
        .map_err(|_| "密文无效".to_string())?;
    String::from_utf8(plain).map_err(|_| "密文无效".into())
}

fn decode_key(key_b64: &str) -> Result<Vec<u8>, String> {
    let bytes = URL_SAFE
        .decode(key_b64)
        .map_err(|_| "FEISHU_CREDENTIAL_KEY 无效".to_string())?;
    if bytes.len() == 32 {
        Ok(bytes)
    } else {
        Err("FEISHU_CREDENTIAL_KEY 无效".into())
    }
}

fn hash_code(code: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(code.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn session_token() -> Result<String, PersonalError> {
    Ok(URL_SAFE
        .encode(random_bytes(16)?)
        .trim_end_matches('=')
        .to_string())
}

fn random_bytes(n: usize) -> Result<Vec<u8>, PersonalError> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf)
        .map_err(|_| PersonalError::Upstream("无法生成注册会话".into()))?;
    Ok(buf)
}

fn text(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn first(body: &Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|key| text(body, key))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn clip(value: &str) -> String {
    value.chars().take(300).collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> String {
        URL_SAFE.encode([9u8; 32])
    }

    #[tokio::test]
    async fn scan_session_hides_the_device_code_and_issues_a_bind_command() {
        let _env_lock = TEST_ENV_LOCK
            .get_or_init(|| async { tokio::sync::Mutex::new(()) })
            .await
            .lock()
            .await;
        std::env::set_var("FEISHU_PERSONAL_LISTENER", "0");
        std::env::set_var("FEISHU_CREDENTIAL_KEY", key());
        let path = std::env::temp_dir().join(format!(
            "vpush-fs-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        db.set_user_text(user, "feishu_open_id", "ou_shared")
            .await
            .unwrap();
        let body = json!({
            "device_code": "device-secret",
            "verification_uri": "https://accounts.feishu.cn/verify",
            "verification_uri_complete": "https://accounts.feishu.cn/verify?code=1",
            "expires_in": 3600,
            "interval": 5
        });
        let started = parse_begin(&body).unwrap();
        let session_id = "sess-1";
        db.create_feishu_session(
            session_id,
            user,
            &seal(&key(), &started.device_code).unwrap(),
            BASE,
            &started.verification_uri,
            10_000,
            5,
        )
        .await
        .unwrap();
        let stored = db.feishu_session(session_id).await.unwrap().unwrap();
        assert_ne!(stored.device_code_ciphertext, "device-secret");
        assert_eq!(
            open_secret(&key(), &stored.device_code_ciphertext).unwrap(),
            "device-secret"
        );
        assert!(!apply_poll(
            &db,
            session_id,
            &json!({"error": "authorization_pending"}),
            1_000
        )
        .await
        .unwrap());
        let ready = json!({
            "client_id": "cli_abcdefgh",
            "client_secret": "sec",
            "user_info": {"open_id": "ou_1", "tenant_brand": "feishu"}
        });
        assert!(apply_poll(&db, session_id, &ready, 1_000).await.unwrap());
        let view = status(&db, user, session_id, 1_000).await.unwrap();
        assert_eq!(view["status"], "awaiting_bind");
        assert_eq!(
            view["verification_uri"],
            "https://accounts.feishu.cn/verify?code=1"
        );
        assert!(view["bind_command"].as_str().unwrap().starts_with("/bind "));
        assert_eq!(
            view["bind_command"].as_str().unwrap().len(),
            6 + "/bind ".len()
        );
        let stored = db.feishu_session(session_id).await.unwrap().unwrap();
        assert_eq!(stored.expected_open_id, "ou_1");
        assert_eq!(stored.candidate_tenant_brand, "feishu");
        assert!(!stored.bind_code_hash.is_empty());
        assert!(!stored.candidate_app_secret_ciphertext.is_empty());
        assert_eq!(view["candidate_app_id_masked"], "cli_abcd…");
        assert!(view["qr_uri"].as_str().unwrap().is_empty());
        let secret: String = sqlx::query_scalar("SELECT candidate_app_secret_ciphertext FROM feishu_registration_sessions WHERE session_id = ?")
            .bind(session_id).fetch_one(db.pool()).await.unwrap();
        assert_ne!(secret, "sec");
        let refreshed = refresh_code(&db, user, session_id, 1_100).await.unwrap();
        assert_ne!(refreshed["bind_command"], view["bind_command"]);
        let code = refreshed["bind_command"]
            .as_str()
            .unwrap()
            .trim_start_matches("/bind ");
        assert!(parse_bind_code("你好").is_none());
        assert!(!accept_bind(
            &db,
            session_id,
            "000000",
            "ou_1",
            "oc_1",
            1_100,
            |_, _, _, _| Ok(())
        )
        .await
        .unwrap());
        assert_eq!(
            db.feishu_session(session_id).await.unwrap().unwrap().status,
            "awaiting_bind"
        );
        assert!(!accept_bind(
            &db,
            session_id,
            code,
            "ou_other",
            "oc_1",
            1_100,
            |_, _, _, _| Ok(())
        )
        .await
        .unwrap());
        let content = serde_json::json!({"text": format!("@_user_1 /bind {code}")}).to_string();
        let payload = serde_json::json!({
            "header": {"event_type": "im.message.receive_v1"},
            "event": {
                "sender": {"sender_id": {"open_id": "ou_1"}},
                "message": {"message_type": "text", "chat_id": "oc_chat", "content": content}
            }
        });
        let reply = crate::feishu_ws::handle_frame(
            &db,
            session_id,
            &crate::feishu_ws::event_frame(&serde_json::to_vec(&payload).unwrap()),
            1_100,
            |app, secret, chat, host| {
                assert_eq!(app, "cli_abcdefgh");
                assert_eq!(secret, "sec");
                assert_eq!(chat, "oc_chat");
                assert_eq!(host, "open.feishu.cn");
                Ok(())
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(reply.windows(12).any(|item| item == br#"{"code":200}"#));
        assert_eq!(
            db.feishu_session(session_id).await.unwrap().unwrap().status,
            "active"
        );
        let bot = db.feishu_bot(user).await.unwrap().unwrap();
        assert_eq!(bot.status, "active");
        assert_eq!(bot.app_id, "cli_abcdefgh");
        let stored_secret: String = sqlx::query_scalar(
            "SELECT app_secret_ciphertext FROM feishu_personal_bots WHERE user_id = ?",
        )
        .bind(user)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_ne!(stored_secret, "sec");
        assert!(!accept_bind(
            &db,
            session_id,
            code,
            "ou_1",
            "oc_chat",
            1_100,
            |_, _, _, _| Ok(())
        )
        .await
        .unwrap());
        assert!(status(&db, user + 1, session_id, 1_100).await.is_err());
        db.create_feishu_session(
            "sess-cancel",
            user,
            "cipher",
            BASE,
            "https://accounts.feishu.cn/verify",
            10_000,
            5,
        )
        .await
        .unwrap();
        db.save_feishu_bind_code("sess-cancel", "hash", 2_000)
            .await
            .unwrap();
        cancel(&db, user, "sess-cancel").await.unwrap();
        assert_eq!(
            db.feishu_session("sess-cancel")
                .await
                .unwrap()
                .unwrap()
                .status,
            "cancelled"
        );
        disable(&db, user).await.unwrap();
        assert!(db.feishu_bot(user).await.unwrap().is_none());
        let shared: String = sqlx::query_scalar("SELECT feishu_open_id FROM users WHERE id = ?")
            .bind(user)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(shared, "ou_shared");
        let _ = std::fs::remove_file(&path);
        std::env::remove_var("FEISHU_CREDENTIAL_KEY");
        std::env::remove_var("FEISHU_PERSONAL_LISTENER");
    }
}
