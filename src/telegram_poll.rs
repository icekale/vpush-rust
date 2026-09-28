use std::fs::{File, OpenOptions, TryLockError};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::db::Db;
use crate::telegram_adapter::{dispatch, parse_update, BotUpdate};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const GET_UPDATES_TIMEOUT: i64 = 30;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(40);
const LEASE_SECS: i64 = 90;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const MAX_BACKOFF_SECS: u64 = 30;

type TransportFuture = Pin<Box<dyn Future<Output = Result<(u16, String), String>> + Send>>;
type Transport = Arc<dyn Fn(String, Value) -> TransportFuture + Send + Sync>;

pub(crate) fn enabled(inbound: Option<&str>, token: Option<&str>) -> bool {
    inbound == Some("1") && token.is_some_and(|token| !token.trim().is_empty())
}

pub(crate) fn spawn(db: Db, token: String, db_path: &Path) -> Option<JoinHandle<()>> {
    let Some(lock) = acquire_os_lock(db_path) else {
        tracing::info!("Telegram 入站 worker 未取得进程锁");
        return None;
    };
    let owner = new_owner();
    Some(tokio::spawn(async move {
        run_with_endpoint(db, token, owner, TELEGRAM_API_BASE, lock).await;
    }))
}

fn lock_path(db_path: &Path) -> Result<PathBuf, String> {
    let canonical = db_path
        .canonicalize()
        .map_err(|_| "Telegram 入站进程锁路径不可用".to_owned())?;
    let stem = canonical
        .file_stem()
        .ok_or_else(|| "Telegram 入站进程锁路径不可用".to_owned())?;
    Ok(canonical.with_file_name(format!("{}.telegram.lock", stem.to_string_lossy())))
}

fn acquire_os_lock(db_path: &Path) -> Option<File> {
    let path = match lock_path(db_path) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!("{error}");
            return None;
        }
    };
    let file = match open_lock_file(&path).and_then(prepare_lock_file) {
        Ok(file) => file,
        Err(_) => {
            tracing::warn!("Telegram 入站进程锁无法安全初始化");
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Error(_)) => {
            tracing::warn!("Telegram 入站进程锁无法获取");
            None
        }
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    options.open(path)
}

fn prepare_lock_file(file: File) -> io::Result<File> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Telegram lock is not a regular file",
        ));
    }
    set_lock_mode(&file)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Telegram lock is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn set_lock_mode(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o600);
    file.set_permissions(permissions)?;
    let mode = file.metadata()?.permissions().mode() & 0o7777;
    if mode != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Telegram lock mode is not 0600",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_lock_mode(_file: &File) -> io::Result<()> {
    Ok(())
}

fn new_owner() -> String {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).expect("OS randomness unavailable");
    hex::encode(bytes)
}

pub(crate) async fn run_with_endpoint(
    db: Db,
    token: String,
    owner: String,
    endpoint: &str,
    _os_lock: File,
) {
    let Some(mut offset) = wait_for_poll_lease(&db, &owner).await else {
        return;
    };

    let (lease_tx, lease_rx) = watch::channel(false);
    let heartbeat = spawn_heartbeat(db.clone(), owner.clone(), lease_tx.clone());
    let result = poll_loop(&db, &token, &owner, endpoint, &mut offset, lease_rx).await;
    lease_tx.send_replace(true);
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

async fn wait_for_poll_lease(db: &Db, owner: &str) -> Option<i64> {
    loop {
        match db.telegram_poll_acquire(owner, LEASE_SECS).await {
            Ok(Some(offset)) => return Some(offset),
            Ok(None) => {
                tracing::info!("Telegram 入站租约被占用，稍后重试");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(error) => {
                tracing::error!("Telegram 入站租约初始化失败: {error}");
                return None;
            }
        }
    }
}

fn spawn_heartbeat(db: Db, owner: String, lease_lost: watch::Sender<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            match db.telegram_poll_heartbeat(&owner, LEASE_SECS).await {
                Ok(true) => {}
                Ok(false) => {
                    lease_lost.send_replace(true);
                    return;
                }
                Err(error) => {
                    tracing::warn!("Telegram 入站租约心跳失败: {error}");
                    lease_lost.send_replace(true);
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
    mut lease: watch::Receiver<bool>,
) -> Result<(), PollError> {
    loop {
        ensure_lease(db, owner, &lease).await?;
        let token_for_request = token.to_owned();
        let endpoint_for_request = endpoint.to_owned();
        let requested_offset = *offset;
        let response = get_updates_with_lease(
            endpoint_for_request,
            token_for_request,
            requested_offset,
            &mut lease,
        )
        .await?;
        match response {
            GetUpdates::Conflict => return Err(PollError::Conflict),
            GetUpdates::RateLimited(seconds) => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
                    changed = lease.changed() => {
                        changed.map_err(|_| PollError::LostLease)?;
                        return Err(PollError::LostLease);
                    }
                }
            }
            GetUpdates::Updates(updates) => {
                for value in updates {
                    *offset = process_update(
                        db.clone(),
                        owner,
                        value,
                        lease.clone(),
                        http_transport(endpoint, token),
                    )
                    .await?;
                }
            }
        }
    }
}

async fn process_update(
    db: Db,
    owner: &str,
    value: Value,
    lease: watch::Receiver<bool>,
    transport: Transport,
) -> Result<i64, PollError> {
    ensure_lease(&db, owner, &lease).await?;
    let update_id = value
        .get("update_id")
        .and_then(Value::as_i64)
        .ok_or(PollError::MalformedUpdate)?;
    if update_id < 0 {
        return Err(PollError::MalformedUpdate);
    }
    let next_offset = update_id.checked_add(1).ok_or(PollError::OffsetOverflow)?;
    let parsed = parse_update(&value).map_err(|_| PollError::MalformedUpdate)?;
    if let Some(update) = parsed {
        dispatch_with_lease(&db, owner, update, lease.clone(), transport).await?;
    }
    ensure_lease(&db, owner, &lease).await?;
    if !db
        .telegram_poll_save_offset(owner, next_offset)
        .await
        .map_err(PollError::Db)?
    {
        return Err(PollError::LostLease);
    }
    Ok(next_offset)
}

async fn get_updates_with_lease(
    endpoint: String,
    token: String,
    offset: i64,
    lease: &mut watch::Receiver<bool>,
) -> Result<GetUpdates, PollError> {
    let request = tokio::task::spawn_blocking(move || get_updates(&endpoint, &token, offset));
    tokio::pin!(request);
    tokio::select! {
        response = &mut request => response
            .map_err(|_| PollError::Http("Telegram request task stopped".into()))?,
        changed = lease.changed() => {
            changed.map_err(|_| PollError::LostLease)?;
            Err(PollError::LostLease)
        }
    }
}

async fn ensure_lease(
    db: &Db,
    owner: &str,
    lease: &watch::Receiver<bool>,
) -> Result<(), PollError> {
    if *lease.borrow() {
        return Err(PollError::LostLease);
    }
    if !db
        .telegram_poll_is_owner(owner)
        .await
        .map_err(PollError::Db)?
    {
        return Err(PollError::LostLease);
    }
    if *lease.borrow() {
        return Err(PollError::LostLease);
    }
    Ok(())
}

async fn dispatch_with_lease(
    db: &Db,
    owner: &str,
    update: BotUpdate,
    mut lease: watch::Receiver<bool>,
    transport: Transport,
) -> Result<(), PollError> {
    ensure_lease(db, owner, &lease).await?;
    let dispatch_db = db.clone();
    let transport_db = dispatch_db.clone();
    let dispatch_owner = owner.to_owned();
    let transport_for_dispatch = transport.clone();
    let lease_for_transport = lease.clone();
    let dispatch = dispatch(&dispatch_db, update, unix_now(), move |method, body| {
        let db = transport_db.clone();
        let owner = dispatch_owner.clone();
        let transport = transport_for_dispatch.clone();
        let lease = lease_for_transport.clone();
        Box::pin(async move {
            ensure_lease(&db, &owner, &lease)
                .await
                .map_err(|error| error.to_string())?;
            transport(method, body).await
        })
    });
    tokio::pin!(dispatch);
    tokio::select! {
        result = &mut dispatch => result.map_err(PollError::Dispatch),
        changed = lease.changed() => {
            changed.map_err(|_| PollError::LostLease)?;
            Err(PollError::LostLease)
        }
    }
}

fn http_transport(endpoint: &str, token: &str) -> Transport {
    let endpoint = endpoint.to_owned();
    let token = token.to_owned();
    Arc::new(move |method, body| {
        let endpoint = endpoint.clone();
        let token = token.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || call_api(&endpoint, &token, &method, body))
                .await
                .map_err(|_| "Telegram request task stopped".to_string())?
        })
    })
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
    MalformedUpdate,
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
            Self::MalformedUpdate => formatter.write_str("Telegram update was malformed"),
            Self::Dispatch(error) => write!(formatter, "Telegram update dispatch failed: {error}"),
            Self::Db(error) => write!(formatter, "Telegram polling database error: {error}"),
            Self::Http(error) => formatter.write_str(error),
            Self::Json(error) => write!(formatter, "Telegram response was invalid: {error}"),
        }
    }
}

fn get_updates(endpoint: &str, token: &str, offset: i64) -> Result<GetUpdates, PollError> {
    let body = json!({
        "offset": offset,
        "timeout": GET_UPDATES_TIMEOUT,
        "allowed_updates": ["message", "callback_query"],
    });
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use tokio::sync::{oneshot, watch};

    fn noop_transport() -> Transport {
        Arc::new(|_method, _body| Box::pin(async { Ok((200, r#"{"ok":true}"#.to_owned())) }))
    }

    #[tokio::test]
    async fn malformed_updates_preserve_cursor_but_known_groups_advance_it() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        db.telegram_poll_acquire("owner", 60).await.unwrap();
        let (_lease_tx, lease_rx) = watch::channel(false);
        let malformed = json!({
            "update_id": 20,
            "message": {"chat": {"id": 42, "type": "private"}}
        });
        assert!(matches!(
            process_update(
                db.clone(),
                "owner",
                malformed,
                lease_rx.clone(),
                noop_transport()
            )
            .await,
            Err(PollError::MalformedUpdate)
        ));
        assert_eq!(db.telegram_poll_offset().await.unwrap(), 0);
        let unknown = json!({"update_id": 21, "edited_message": {}});
        assert!(matches!(
            process_update(
                db.clone(),
                "owner",
                unknown,
                lease_rx.clone(),
                noop_transport()
            )
            .await,
            Err(PollError::MalformedUpdate)
        ));
        assert_eq!(db.telegram_poll_offset().await.unwrap(), 0);
        let group = json!({
            "update_id": 22,
            "message": {"chat": {"id": -4, "type": "group"}}
        });
        assert_eq!(
            process_update(db.clone(), "owner", group, lease_rx, noop_transport())
                .await
                .unwrap(),
            23
        );
        assert_eq!(db.telegram_poll_offset().await.unwrap(), 23);
    }

    #[tokio::test]
    async fn lease_loss_aborts_paused_dispatch_without_cursor_save_or_next_call() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        db.telegram_poll_acquire("old-owner", 60).await.unwrap();
        let (lease_tx, lease_rx) = watch::channel(false);
        let (entered_tx, entered_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel::<()>();
        let entered = Arc::new(Mutex::new(Some(entered_tx)));
        let resume = Arc::new(Mutex::new(Some(resume_rx)));
        let calls = Arc::new(AtomicUsize::new(0));
        let transport: Transport = {
            let entered = entered.clone();
            let resume = resume.clone();
            let calls = calls.clone();
            Arc::new(move |_method, _body| {
                calls.fetch_add(1, Ordering::SeqCst);
                let entered = entered.lock().unwrap().take();
                let resume = resume.lock().unwrap().take();
                Box::pin(async move {
                    if let Some(entered) = entered {
                        let _ = entered.send(());
                    }
                    if let Some(resume) = resume {
                        let _ = resume.await;
                    }
                    Ok((200, r#"{"ok":true}"#.to_owned()))
                })
            })
        };
        let update = json!({
            "update_id": 30,
            "message": {
                "chat": {"id": 42, "type": "private", "first_name": "Ada"},
                "text": "/help"
            }
        });
        let task = tokio::spawn(process_update(
            db.clone(),
            "old-owner",
            update,
            lease_rx,
            transport,
        ));
        entered_rx.await.unwrap();
        sqlx::query("UPDATE telegram_poll_state SET lease_until = unixepoch() - 1")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            db.telegram_poll_acquire("new-owner", 60).await.unwrap(),
            Some(0)
        );
        lease_tx.send(true).unwrap();
        assert!(matches!(task.await.unwrap(), Err(PollError::LostLease)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(db.telegram_poll_offset().await.unwrap(), 0);
        drop(resume_tx);
    }

    #[cfg(unix)]
    #[test]
    fn existing_lock_mode_is_hardened_to_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "vpush-telegram-lock-mode-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("vpush.db");
        std::fs::write(&db_path, b"db").unwrap();
        let path = lock_path(&db_path).unwrap();
        std::fs::write(&path, b"lock").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let file = acquire_os_lock(&db_path).unwrap();
        assert_eq!(
            file.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );
        drop(file);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_lock_is_refused_without_touching_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = std::env::temp_dir().join(format!(
            "vpush-telegram-lock-symlink-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("vpush.db");
        std::fs::write(&db_path, b"db").unwrap();
        let target = dir.join("target");
        std::fs::write(&target, b"target").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o666)).unwrap();
        symlink(&target, lock_path(&db_path).unwrap()).unwrap();
        assert!(acquire_os_lock(&db_path).is_none());
        assert_eq!(std::fs::read(&target).unwrap(), b"target");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
            0o666
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn sidecar_lock_fences_sql_lease_steal_until_first_worker_drops_it() {
        let dir = std::env::temp_dir().join(format!(
            "vpush-telegram-lock-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("vpush.db");
        let db = Db::open(&db_path).await.unwrap();
        let first_lock = acquire_os_lock(&db_path).unwrap();
        db.telegram_poll_acquire("first", 60).await.unwrap();
        sqlx::query("UPDATE telegram_poll_state SET lease_until = unixepoch() - 1")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            db.telegram_poll_acquire("second", 60).await.unwrap(),
            Some(0)
        );
        assert!(acquire_os_lock(&db_path).is_none());
        drop(first_lock);
        assert!(acquire_os_lock(&db_path).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn normal_worker_failure_releases_sidecar_lock() {
        let dir = std::env::temp_dir().join(format!(
            "vpush-telegram-lock-failure-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("vpush.db");
        let db = Db::open(&db_path).await.unwrap();
        let lock = acquire_os_lock(&db_path).unwrap();
        run_with_endpoint(
            db,
            "test-token".to_owned(),
            "owner".to_owned(),
            "http://127.0.0.1:1",
            lock,
        )
        .await;
        assert!(acquire_os_lock(&db_path).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

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
    async fn worker_retries_until_existing_lease_expires() {
        let dir = std::env::temp_dir().join(format!(
            "vpush-telegram-lease-retry-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("vpush.db");
        let db = Db::open(&db_path).await.unwrap();
        db.telegram_poll_acquire("old", 60).await.unwrap();
        let waiting = db.clone();
        let worker = tokio::spawn(async move { wait_for_poll_lease(&waiting, "new").await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        sqlx::query("UPDATE telegram_poll_state SET lease_until = unixepoch() - 1")
            .execute(db.pool())
            .await
            .unwrap();
        let offset = tokio::time::timeout(Duration::from_secs(7), worker)
            .await
            .expect("worker gave up while another lease was still held")
            .unwrap();
        assert_eq!(offset, Some(0));
        assert!(db.telegram_poll_is_owner("new").await.unwrap());
        let _ = std::fs::remove_dir_all(dir);
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
