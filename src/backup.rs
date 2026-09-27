//! 管理员数据库备份。只导出 SQLite 文件，不包含附件目录。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{json, Value};

use crate::db::Db;

const URL: &str = "backup_webdav_url";
const USER: &str = "backup_webdav_username";
const PASSWORD: &str = "backup_webdav_password";
const PATH: &str = "backup_webdav_path";
const HOUR: &str = "backup_webdav_hour";
const KEEP: &str = "backup_webdav_keep";
const LAST_OK: &str = "backup_last_ok_at";
const LAST_ERR: &str = "backup_last_error";
const LAST_NAME: &str = "backup_last_remote_name";
const DEFAULT_PATH: &str = "/vpush-backups";
const UPLOAD_MAX: usize = 200 * 1024 * 1024;
const MAGIC: &[u8] = b"VPUSH1\0";

static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub struct BackupError {
    pub status: u16,
    pub detail: &'static str,
}

struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

fn lock() -> Result<Guard, BackupError> {
    if BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err(fail(409, "已有备份或恢复在进行"));
    }
    Ok(Guard)
}

fn fail(status: u16, detail: &'static str) -> BackupError {
    BackupError { status, detail }
}

#[derive(Clone)]
struct Cfg {
    url: String,
    username: String,
    password: String,
    path: String,
    hour: i64,
    keep: i64,
}

pub async fn status(db: &Db) -> Result<Value, BackupError> {
    let cfg = load(db).await?;
    let last_ok = setting(db, LAST_OK).await?;
    Ok(json!({
        "url": cfg.url,
        "username": cfg.username,
        "path": cfg.path,
        "hour": cfg.hour,
        "keep": cfg.keep,
        "password_set": !cfg.password.is_empty(),
        "last_ok_at": last_ok,
        "last_error": setting(db, LAST_ERR).await?,
        "last_remote_name": setting(db, LAST_NAME).await?,
        "next_run_at": next_run(cfg.hour, &last_ok),
    }))
}

pub async fn save(db: &Db, body: &Value) -> Result<Value, BackupError> {
    if let Some(url) = body.get("url").and_then(Value::as_str) {
        let url = url.trim();
        if !url.is_empty() && !valid_https(url) {
            return Err(fail(400, "WebDAV 地址需要 https"));
        }
        db.set_setting(URL, url).await.map_err(|_| fail(500, "保存失败"))?;
    }
    if let Some(username) = body.get("username").and_then(Value::as_str) {
        db.set_setting(USER, username.trim()).await.map_err(|_| fail(500, "保存失败"))?;
    }
    if let Some(password) = body.get("password").and_then(Value::as_str).filter(|text| !text.is_empty()) {
        db.set_setting(PASSWORD, password).await.map_err(|_| fail(500, "保存失败"))?;
    }
    if body.get("path").is_some() {
        let path = body["path"].as_str().unwrap_or("").trim();
        let path = if path.is_empty() { DEFAULT_PATH.to_string() } else if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
        db.set_setting(PATH, &path).await.map_err(|_| fail(500, "保存失败"))?;
    }
    if let Some(hour) = body.get("hour").and_then(Value::as_i64) {
        if !(0..=23).contains(&hour) {
            return Err(fail(400, "每天几点需在 0-23 之间"));
        }
        db.set_setting(HOUR, &hour.to_string()).await.map_err(|_| fail(500, "保存失败"))?;
    } else if body.get("hour").is_some() {
        return Err(fail(400, "每天几点需在 0-23 之间"));
    }
    if let Some(keep) = body.get("keep").and_then(Value::as_i64) {
        if !(1..=90).contains(&keep) {
            return Err(fail(400, "保留份数需在 1-90 之间"));
        }
        db.set_setting(KEEP, &keep.to_string()).await.map_err(|_| fail(500, "保存失败"))?;
    } else if body.get("keep").is_some() {
        return Err(fail(400, "保留份数需在 1-90 之间"));
    }
    status(db).await
}

pub async fn snapshot(db: &Db) -> Result<PathBuf, BackupError> {
    let _guard = lock()?;
    snapshot_unlocked(db).await
}

pub async fn restore_bytes(db: &Db, data: &[u8]) -> Result<(), BackupError> {
    let _guard = lock()?;
    if data.is_empty() || data.len() > UPLOAD_MAX {
        return Err(fail(400, "请上传有效的 .db 备份文件"));
    }
    if data.starts_with(MAGIC) {
        return Err(fail(400, "备份已加密，请配置 FEISHU_CREDENTIAL_KEY 后再恢复"));
    }
    if !data.starts_with(b"SQLite format 3\0") {
        return Err(fail(400, "请上传有效的 .db 备份文件"));
    }
    let live = db_file(db).await?;
    let folder = live.parent().unwrap_or(Path::new(".")).join("backups");
    std::fs::create_dir_all(&folder).map_err(|_| fail(500, "读取数据库失败"))?;
    let candidate = folder.join(format!("restore-{}.db", now_secs()));
    std::fs::write(&candidate, data).map_err(|_| fail(500, "读取数据库失败"))?;
    let result = restore_candidate(db, &live, &candidate).await;
    let _ = std::fs::remove_file(&candidate);
    result
}

pub async fn test_webdav(db: &Db, body: Option<&Value>) -> Result<(), BackupError> {
    let cfg = merged(db, body).await?;
    if cfg.url.is_empty() || cfg.password.is_empty() {
        return Err(fail(400, "先保存 WebDAV 配置"));
    }
    if !valid_https(&cfg.url) {
        return Err(fail(400, "WebDAV 地址需要 https"));
    }
    let folder = join(&cfg.url, &cfg.path);
    let status = dav(&cfg, "PROPFIND", &folder, Some("0"), None)?;
    if status == 404 {
        let created = dav(&cfg, "MKCOL", &folder, None, None)?;
        if !(created == 201 || created == 204 || created == 405) && created >= 400 {
            return Err(fail(400, "WebDAV 连不上，请检查地址和账号"));
        }
        let again = dav(&cfg, "PROPFIND", &folder, Some("0"), None)?;
        if again >= 400 {
            return Err(fail(400, "WebDAV 连不上，请检查地址和账号"));
        }
        return Ok(());
    }
    if status >= 400 {
        return Err(fail(400, "WebDAV 连不上，请检查地址和账号"));
    }
    Ok(())
}

pub async fn restore_webdav(db: &Db) -> Result<(), BackupError> {
    let cfg = load(db).await?;
    if cfg.url.is_empty() || cfg.password.is_empty() {
        return Err(fail(400, "先保存 WebDAV 配置"));
    }
    let folder = join(&cfg.url, &cfg.path);
    let listing = dav_text(&cfg, "PROPFIND", &folder, Some("1"))?;
    let name = backup_names(&listing).into_iter().next_back().ok_or(fail(400, "网盘上还没有备份文件"))?;
    let bytes = dav_bytes(&cfg, &join(&folder, &name))?;
    restore_bytes(db, &bytes).await
}

async fn restore_candidate(db: &Db, live: &Path, candidate: &Path) -> Result<(), BackupError> {
    if !sqlite_ok(candidate) {
        return Err(fail(400, "备份文件损坏，已取消恢复，当前数据未改"));
    }
    let snap = snapshot_unlocked(db).await?;
    if !restore_into(live, candidate) {
        let _ = restore_into(live, &snap);
        return Err(fail(400, "恢复失败，已保持恢复前的数据库"));
    }
    if sqlx::query("SELECT 1").execute(db.pool()).await.is_err() {
        let _ = restore_into(live, &snap);
        return Err(fail(400, "恢复失败，已保持恢复前的数据库"));
    }
    Ok(())
}

async fn snapshot_unlocked(db: &Db) -> Result<PathBuf, BackupError> {
    let live = db_file(db).await?;
    let folder = live.parent().unwrap_or(Path::new(".")).join("backups");
    std::fs::create_dir_all(&folder).map_err(|_| fail(500, "读取数据库失败"))?;
    let target = unique_snapshot(&folder);
    let escaped = target.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'")).execute(db.pool()).await.map_err(|_| fail(500, "备份校验失败，请稍后重试"))?;
    if !sqlite_ok(&target) {
        let _ = std::fs::remove_file(&target);
        return Err(fail(500, "备份校验失败，请稍后重试"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
    }
    prune(&folder);
    Ok(target)
}

fn unique_snapshot(folder: &Path) -> PathBuf {
    let stamp = utc_stamp();
    let mut target = folder.join(format!("dav-{stamp}.db"));
    let mut n = 1;
    while target.exists() {
        target = folder.join(format!("dav-{stamp}-{n}.db"));
        n += 1;
    }
    target
}

fn prune(folder: &Path) {
    let mut files: Vec<_> = std::fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| {
        path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("dav-") && name.ends_with(".db"))
    }).collect();
    files.sort();
    let keep = files.len().saturating_sub(3);
    for path in files.into_iter().take(keep) {
        let _ = std::fs::remove_file(path);
    }
}

fn sqlite_ok(path: &Path) -> bool {
    let output = Command::new("sqlite3").arg(path).arg("PRAGMA quick_check; SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='users') THEN 'yes' ELSE 'no' END;").output();
    let Ok(output) = output else { return false };
    let text = String::from_utf8_lossy(&output.stdout);
    output.status.success() && text.lines().next() == Some("ok") && text.contains("yes")
}

fn restore_into(live: &Path, source: &Path) -> bool {
    if path_unsafe(live) || path_unsafe(source) {
        return false;
    }
    let output = Command::new("sqlite3").arg(live).arg(format!(".restore {}", source.display())).output();
    output.is_ok_and(|output| output.status.success())
}

fn path_unsafe(path: &Path) -> bool {
    path.to_string_lossy().chars().any(|ch| ch.is_whitespace() || ch == '\'' || ch == '"')
}

fn backup_names(xml: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("href>") {
        rest = &rest[start + 5..];
        let end = rest.find('<').unwrap_or(rest.len());
        let href = &rest[..end];
        let name = href.rsplit('/').next().unwrap_or("").trim();
        let name = percent_decode(name);
        if name.starts_with("dav-") && name.ends_with(".db") {
            names.push(name);
        }
        rest = &rest[end..];
    }
    names.sort();
    names.dedup();
    names
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn dav(cfg: &Cfg, method: &str, url: &str, depth: Option<&str>, body: Option<&[u8]>) -> Result<u16, BackupError> {
    let response = dav_call(cfg, method, url, depth, body)?;
    Ok(response.status())
}

fn dav_text(cfg: &Cfg, method: &str, url: &str, depth: Option<&str>) -> Result<String, BackupError> {
    let response = dav_call(cfg, method, url, depth, None)?;
    if response.status() >= 400 {
        return Err(fail(400, "WebDAV 连不上，请检查地址和账号"));
    }
    response.into_string().map_err(|_| fail(400, "WebDAV 连不上，请检查地址和账号"))
}

fn dav_bytes(cfg: &Cfg, url: &str) -> Result<Vec<u8>, BackupError> {
    let response = dav_call(cfg, "GET", url, None, None)?;
    if response.status() == 404 {
        return Err(fail(400, "网盘上还没有备份文件"));
    }
    if response.status() >= 400 {
        return Err(fail(400, "WebDAV 连不上，请检查地址和账号"));
    }
    let mut data = Vec::new();
    response.into_reader().take(UPLOAD_MAX as u64 + 1).read_to_end(&mut data).map_err(|_| fail(400, "WebDAV 连不上，请检查地址和账号"))?;
    if data.len() > UPLOAD_MAX {
        return Err(fail(400, "请上传有效的 .db 备份文件"));
    }
    Ok(data)
}

fn dav_call(cfg: &Cfg, method: &str, url: &str, depth: Option<&str>, body: Option<&[u8]>) -> Result<ureq::Response, BackupError> {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(20)).redirects(0).build();
    let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", cfg.username, cfg.password)));
    let mut request = agent.request(method, url).set("Authorization", &auth);
    if let Some(depth) = depth {
        request = request.set("Depth", depth);
    }
    let result = if let Some(body) = body { request.send_bytes(body) } else { request.call() };
    match result {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(status, response)) => {
            if status == 401 || status == 403 {
                return Err(fail(400, "WebDAV 连不上，请检查地址和账号"));
            }
            Ok(response)
        }
        Err(_) => Err(fail(400, "WebDAV 连不上，请检查地址和账号")),
    }
}

async fn merged(db: &Db, body: Option<&Value>) -> Result<Cfg, BackupError> {
    let mut cfg = load(db).await?;
    let Some(body) = body else { return Ok(cfg) };
    if let Some(url) = body.get("url").and_then(Value::as_str) {
        cfg.url = url.trim().to_string();
    }
    if let Some(username) = body.get("username").and_then(Value::as_str) {
        cfg.username = username.trim().to_string();
    }
    if let Some(password) = body.get("password").and_then(Value::as_str).filter(|text| !text.is_empty()) {
        cfg.password = password.to_string();
    }
    if let Some(path) = body.get("path").and_then(Value::as_str) {
        let path = path.trim();
        cfg.path = if path.is_empty() { DEFAULT_PATH.into() } else if path.starts_with('/') { path.into() } else { format!("/{path}") };
    }
    Ok(cfg)
}

async fn load(db: &Db) -> Result<Cfg, BackupError> {
    let hour = setting(db, HOUR).await?.parse().ok().filter(|hour: &i64| (0..=23).contains(hour)).unwrap_or(3);
    let keep = setting(db, KEEP).await?.parse().ok().filter(|keep: &i64| (1..=90).contains(keep)).unwrap_or(14);
    Ok(Cfg {
        url: setting(db, URL).await?,
        username: setting(db, USER).await?,
        password: setting(db, PASSWORD).await?,
        path: {
            let path = setting(db, PATH).await?;
            if path.is_empty() { DEFAULT_PATH.into() } else { path }
        },
        hour,
        keep,
    })
}

async fn setting(db: &Db, key: &str) -> Result<String, BackupError> {
    db.setting(key).await.map(|value| value.unwrap_or_default()).map_err(|_| fail(500, "读取数据库失败"))
}

async fn db_file(db: &Db) -> Result<PathBuf, BackupError> {
    let file: Option<String> = sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
        .fetch_optional(db.pool())
        .await
        .map_err(|_| fail(500, "读取数据库失败"))?;
    let file = file.unwrap_or_default();
    if file.is_empty() {
        return Err(fail(400, "当前数据库不能导出"));
    }
    Ok(PathBuf::from(file))
}

fn valid_https(url: &str) -> bool {
    let url = url.trim();
    url.starts_with("https://")
        && url.len() <= 500
        && !url.contains('@')
        && !url.chars().any(char::is_control)
        && url["https://".len()..].contains('.')
}

fn join(base: &str, path: &str) -> String {
    let mut url = base.trim().trim_end_matches('/').to_string();
    for part in path.split('/') {
        if !part.is_empty() {
            url.push('/');
            url.push_str(part);
        }
    }
    url
}

fn next_run(hour: i64, last_ok: &str) -> String {
    let now = now_secs();
    let today = utc_stamp()[..10].to_string();
    let mut day = now - (now % 86_400) + hour as u64 * 3600;
    if last_ok.starts_with(&today) {
        day += 86_400;
    }
    let days = day / 86_400;
    let (year, month, date) = civil_date(days);
    format!("{year:04}-{month:02}-{date:02} {hour:02}:00")
}

fn utc_stamp() -> String {
    let secs = now_secs();
    let (year, month, day) = civil_date(secs / 86_400);
    let rem = secs % 86_400;
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn civil_date(days: u64) -> (i32, u32, u32) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year as i32, month as u32, day as u32)
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub async fn run_scheduled(db: &Db) -> Result<bool, BackupError> {
    let cfg = load(db).await?;
    if cfg.url.is_empty() || cfg.password.is_empty() {
        return Ok(true);
    }
    let now = now_secs();
    if (now % 86_400) / 3600 < cfg.hour as u64 {
        return Ok(true);
    }
    let today = &utc_human()[..10];
    if setting(db, LAST_OK).await?.starts_with(today) {
        return Ok(true);
    }
    let path = match snapshot(db).await {
        Ok(path) => path,
        Err(err) if err.status == 409 => return Ok(true),
        Err(err) => {
            let _ = db.set_setting(LAST_ERR, err.detail).await;
            return Ok(false);
        }
    };
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("dav-backup.db").to_string();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = db.set_setting(LAST_ERR, "读取数据库失败").await;
            return Ok(false);
        }
    };
    let folder = join(&cfg.url, &cfg.path);
    if let Err(err) = upload_pruned(&cfg, &folder, &name, &bytes) {
        let _ = db.set_setting(LAST_ERR, err.detail).await;
        return Ok(false);
    }
    let _ = db.set_setting(LAST_OK, &utc_human()).await;
    let _ = db.set_setting(LAST_ERR, "").await;
    let _ = db.set_setting(LAST_NAME, &name).await;
    Ok(true)
}

fn upload_pruned(cfg: &Cfg, folder: &str, name: &str, bytes: &[u8]) -> Result<(), BackupError> {
    let status = dav(cfg, "PUT", &join(folder, name), None, Some(bytes))?;
    if status >= 400 {
        return Err(fail(400, "WebDAV 连不上，请检查地址和账号"));
    }
    if let Ok(listing) = dav_text(cfg, "PROPFIND", folder, Some("1")) {
        let names = backup_names(&listing);
        let keep = cfg.keep.max(1) as usize;
        if names.len() > keep {
            for old in &names[..names.len() - keep] {
                let _ = dav(cfg, "DELETE", &join(folder, old), None, None);
            }
        }
    }
    Ok(())
}

fn utc_human() -> String {
    let secs = now_secs();
    let (year, month, day) = civil_date(secs / 86_400);
    let rem = secs % 86_400;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_webdav_listing() {
        let xml = r#"<D:href>/vpush-backups/</D:href><D:href>/vpush-backups/dav-20260814-030000.db</D:href><D:href>/x/dav-20260813-030000.db</D:href>"#;
        assert_eq!(backup_names(xml), vec!["dav-20260813-030000.db".to_string(), "dav-20260814-030000.db".to_string()]);
    }

    #[tokio::test]
    async fn saves_config_downloads_and_restores_without_showing_the_password() {
        let dir = std::env::temp_dir().join(format!("vpush-bak-{}-{}", std::process::id(), now_secs()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.db");
        let db = Db::open(&path).await.unwrap();
        let saved = save(&db, &json!({"url":"http://example.com","username":"u","password":"secret","path":"vpush","hour":4,"keep":7})).await.unwrap_err();
        assert_eq!(saved.detail, "WebDAV 地址需要 https");
        let status = save(&db, &json!({"url":"https://dav.example.com/remote.php/webdav","username":"u","password":"secret","path":"vpush","hour":4,"keep":7})).await.unwrap();
        assert_eq!(status["password_set"], true);
        assert!(status.get("password").is_none());
        assert_eq!(status["path"], "/vpush");
        let kept = save(&db, &json!({"url":"https://dav.example.com/remote.php/webdav","username":"u","hour":4,"keep":7})).await.unwrap();
        assert_eq!(kept["password_set"], true);
        db.set_setting("backup_marker", "before").await.unwrap();
        let snap = snapshot(&db).await.unwrap();
        let bytes = std::fs::read(&snap).unwrap();
        db.set_setting("backup_marker", "after").await.unwrap();
        restore_bytes(&db, &bytes).await.unwrap();
        assert_eq!(db.setting("backup_marker").await.unwrap().as_deref(), Some("before"));
        assert!(restore_bytes(&db, b"not a database").await.is_err());
        assert_eq!(db.setting("backup_marker").await.unwrap().as_deref(), Some("before"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
