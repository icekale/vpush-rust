use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::db::Db;
use crate::telegram_adapter::{dispatch, parse_update};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const GET_UPDATES_TIMEOUT: i64 = 30;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(40);
const LEASE_SECS: i64 = 90;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const MAX_BACKOFF_SECS: u64 = 30;

pub(crate) fn enabled(inbound: Option<&str>, token: Option<&str>) -> bool {
    inbound == Some("1") && token.is_some_and(|token| !token.trim().is_empty())
}

pub(crate) fn spawn(db: Db, token: String) -> JoinHandle<()> {
    let owner = new_owner();
    tokio::spawn(async move {
        run_with_endpoint(db, token, owner, TELEGRAM_API_BASE).await;
    })
}

fn new_owner() -> String {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).expect("OS randomness unavailable");
    hex::encode(bytes)
}

pub(crate) async fn run_with_endpoint(db: Db, token: String, owner: String, endpoint: &str) {
    let Some(mut offset) = (match db.telegram_poll_acquire(&owner, LEASE_SECS).await {
        Ok(offset) => offset,
        Err(error) => {
            tracing::error!("Telegram 入站租约初始化失败: {error}");
            return;
        }
    }) else {
        tracing::info!("Telegram 入站 worker 未取得租约");
        return;
    };

    let lost = Arc::new(AtomicBool::new(false));
    let heartbeat = spawn_heartbeat(db.clone(), owner.clone(), lost.clone());
    let result = poll_loop(&db, &token, &owner, endpoint, &mut offset, lost.clone()).await;
    lost.store(true, Ordering::Release);
    heartbeat.abort();
    if let Err(error) = db.telegram_poll_release(&owner).await {
        tracing::warn!("Telegram 入站租约释放失败: {error}");
    }
    if let Err(error) = result {
        match error {
            PollError::Conflict | PollError::LostLease => {
                tracing::warn!("Telegram 入站 worker 停止：租约已失效")
            }
            other => tracing::warn!("Telegram 入站 worker 停止: {other}"),
        }
    }
}

fn spawn_heartbeat(db: Db, owner: String, lost: Arc<AtomicBool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            if lost.load(Ordering::Acquire) {
                return;
            }
            match db.telegram_poll_heartbeat(&owner, LEASE_SECS).await {
                Ok(true) => {}
                Ok(false) => {
                    lost.store(true, Ordering::Release);
                    return;
                }
                Err(error) => {
                    tracing::warn!("Telegram 入站租约心跳失败: {error}");
                    lost.store(true, Ordering::Release);
                    return;
                }
            }
        }
    })
}

async fn poll_loop(
    db: &Db,
    token: &str,
    owner: &str,
    endpoint: &str,
    offset: &mut i64,
    lost: Arc<AtomicBool>,
) -> Result<(), PollError> {
    loop {
        if lost.load(Ordering::Acquire) {
            return Err(PollError::LostLease);
        }
        let token_for_request = token.to_owned();
        let endpoint_for_request = endpoint.to_owned();
        let requested_offset = *offset;
        let response = tokio::task::spawn_blocking(move || {
            get_updates(&endpoint_for_request, &token_for_request, requested_offset)
        })
        .await
        .map_err(|_| PollError::Http("Telegram request task stopped".into()))??;
        match response {
            GetUpdates::Conflict => return Err(PollError::Conflict),
            GetUpdates::RateLimited(seconds) => {
                tokio::time::sleep(Duration::from_secs(seconds)).await;
                continue;
            }
            GetUpdates::Updates(updates) => {
                for value in updates {
                    if lost.load(Ordering::Acquire) {
                        return Err(PollError::LostLease);
                    }
                    let Some(update_id) = value.get("update_id").and_then(Value::as_i64) else {
                        continue;
                    };
                    if update_id < 0 {
                        continue;
                    }
                    let next_offset = update_id.checked_add(1).ok_or(PollError::OffsetOverflow)?;
                    if let Ok(Some(update)) = parse_update(&value) {
                        let db = db.clone();
                        let dispatch_token = token.to_owned();
                        let dispatch_endpoint = endpoint.to_owned();
                        dispatch(&db, update, unix_now(), move |method, body| {
                            let token = dispatch_token.clone();
                            let endpoint = dispatch_endpoint.clone();
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    call_api(&endpoint, &token, &method, body)
                                })
                                .await
                                .map_err(|_| "Telegram request task stopped".to_string())?
                            }
                        })
                        .await
                        .map_err(PollError::Dispatch)?;
                    }
                    if !db
                        .telegram_poll_save_offset(owner, next_offset)
                        .await
                        .map_err(PollError::Db)?
                    {
                        return Err(PollError::LostLease);
                    }
                    *offset = next_offset;
                }
            }
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

enum GetUpdates {
    Updates(Vec<Value>),
    RateLimited(u64),
    Conflict,
}

#[derive(Debug)]
enum PollError {
    Conflict,
    LostLease,
    OffsetOverflow,
    Dispatch(String),
    Db(sqlx::Error),
    Http(String),
    Json(String),
}

impl std::fmt::Display for PollError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => formatter.write_str("Telegram reported a polling conflict"),
            Self::LostLease => formatter.write_str("polling lease was lost"),
            Self::OffsetOverflow => formatter.write_str("Telegram update_id overflowed cursor"),
            Self::Dispatch(error) => write!(formatter, "Telegram update dispatch failed: {error}"),
            Self::Db(error) => write!(formatter, "Telegram polling database error: {error}"),
            Self::Http(error) => formatter.write_str(error),
            Self::Json(error) => write!(formatter, "Telegram response was invalid: {error}"),
        }
    }
}

fn get_updates(endpoint: &str, token: &str, offset: i64) -> Result<GetUpdates, PollError> {
    let body = json!({"offset": offset, "timeout": GET_UPDATES_TIMEOUT});
    let (status, response) = request(endpoint, token, "getUpdates", body)?;
    let value: Value =
        serde_json::from_str(&response).map_err(|error| PollError::Json(error.to_string()))?;
    let retry_after = value
        .get("parameters")
        .and_then(|parameters| parameters.get("retry_after"))
        .and_then(Value::as_u64);
    if status == 409
        || value["description"]
            .as_str()
            .is_some_and(|description| description.contains("conflict"))
    {
        return Ok(GetUpdates::Conflict);
    }
    if status == 429 || retry_after.is_some() {
        return Ok(GetUpdates::RateLimited(
            retry_after.unwrap_or(1).clamp(1, MAX_BACKOFF_SECS),
        ));
    }
    if status / 100 != 2 || value.get("ok") != Some(&Value::Bool(true)) {
        return Err(PollError::Http("Telegram API request failed".into()));
    }
    let results = value
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| PollError::Json("result was not an array".into()))?;
    Ok(GetUpdates::Updates(results.clone()))
}

fn call_api(
    endpoint: &str,
    token: &str,
    method: &str,
    body: Value,
) -> Result<(u16, String), String> {
    request(endpoint, token, method, body).map_err(|error| error.to_string())
}

fn request(
    endpoint: &str,
    token: &str,
    method: &str,
    body: Value,
) -> Result<(u16, String), PollError> {
    let url = format!("{}/bot{}/{}", endpoint.trim_end_matches('/'), token, method);
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    match agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(
            &serde_json::to_string(&body)
                .map_err(|_| PollError::Http("Telegram request could not be encoded".into()))?,
        ) {
        Ok(response) => {
            let status = response.status();
            let body = response
                .into_string()
                .map_err(|_| PollError::Http("Telegram response body could not be read".into()))?;
            Ok((status, body))
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            Ok((status, body))
        }
        Err(_) => Err(PollError::Http("Telegram HTTP request failed".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::telegram_adapter::{dispatch, parse_update};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread;

    #[tokio::test]
    async fn lease_allows_one_owner_and_release_is_owner_checked() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        assert_eq!(db.telegram_poll_acquire("one", 60).await.unwrap(), Some(0));
        assert_eq!(db.telegram_poll_acquire("two", 60).await.unwrap(), None);
        assert!(!db.telegram_poll_release("two").await.unwrap());
        assert!(db.telegram_poll_release("one").await.unwrap());
        assert_eq!(db.telegram_poll_acquire("two", 60).await.unwrap(), Some(0));
    }

    #[tokio::test]
    async fn expired_lease_can_be_acquired_by_another_owner() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        db.telegram_poll_acquire("one", 60).await.unwrap();
        sqlx::query("UPDATE telegram_poll_state SET lease_until = unixepoch() - 1")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(db.telegram_poll_acquire("two", 60).await.unwrap(), Some(0));
    }

    #[tokio::test]
    async fn cursor_survives_restart_and_failed_dispatch_does_not_advance_it() {
        let path = std::env::temp_dir().join(format!(
            "vpush-telegram-poll-{}-{}.db",
            std::process::id(),
            unix_now()
        ));
        let db = Db::open(&path).await.unwrap();
        db.telegram_poll_acquire("owner", 60).await.unwrap();
        let update = parse_update(&json!({
            "update_id": 7,
            "message": {
                "chat": {"id": 42, "type": "private", "first_name": "Ada"},
                "text": "/help"
            }
        }))
        .unwrap()
        .unwrap();
        let error = dispatch(&db, update, 1_000, |_method, _body| async {
            Err::<(u16, String), _>("send failed".to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(error, "send failed");
        assert_eq!(db.telegram_poll_offset().await.unwrap(), 0);
        assert!(db.telegram_poll_save_offset("owner", 8).await.unwrap());
        assert!(db.telegram_poll_release("owner").await.unwrap());
        drop(db);

        let restarted = Db::open(&path).await.unwrap();
        assert_eq!(restarted.telegram_poll_offset().await.unwrap(), 8);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn inbound_gate_requires_explicit_flag_and_token() {
        assert!(!enabled(None, Some("123:secret")));
        assert!(!enabled(Some("1"), None));
        assert!(!enabled(Some("1"), Some("  ")));
        assert!(enabled(Some("1"), Some("123:secret")));
    }

    #[test]
    fn api_errors_do_not_include_token() {
        let token = "123456:super-secret-token";
        let error = request("http://127.0.0.1:1", token, "getUpdates", json!({}))
            .unwrap_err()
            .to_string();
        assert!(!error.contains(token));
        assert!(!error.contains("super-secret"));
    }

    #[test]
    fn fake_api_accepts_injected_endpoint_without_real_telegram() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let length = stream.read(&mut chunk).unwrap();
                if length == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..length]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("POST /bot123:secret/getUpdates"));
            let response = r#"{"ok":true,"result":[]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        });
        let result = get_updates(&format!("http://{}", address), "123:secret", 0).unwrap();
        assert!(matches!(result, GetUpdates::Updates(updates) if updates.is_empty()));
        thread.join().unwrap();
    }
}
