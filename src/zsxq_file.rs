use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Semaphore;

use crate::db::Db;

const MAX_BYTES: usize = 80 * 1024 * 1024;
const API: &str = "https://api.zsxq.com/v2/files";

#[derive(Debug)]
pub struct FileError {
    pub status: u16,
    pub detail: String,
}

pub struct Downloaded {
    pub bytes: Vec<u8>,
    pub disposition: String,
}

pub async fn read_file(
    db: &Db,
    user_id: i64,
    is_admin: bool,
    file_id: &str,
) -> Result<Downloaded, FileError> {
    if !file_id.chars().all(|ch| ch.is_ascii_digit()) || file_id.is_empty() || file_id.len() > 32 {
        return Err(fail(400, "无效附件"));
    }
    let hits = db
        .zsxq_file_posts(file_id)
        .await
        .map_err(|_| fail(500, "读取附件失败"))?;
    if hits.is_empty() {
        return Err(fail(404, "附件不存在"));
    }
    if !is_admin {
        let mut allowed = false;
        for (_, kol_id, _) in &hits {
            if db
                .is_subscribed(user_id, *kol_id)
                .await
                .map_err(|_| fail(500, "读取附件失败"))?
            {
                allowed = true;
                break;
            }
        }
        if !allowed {
            return Err(fail(404, "附件不存在"));
        }
    }
    let name = file_name(&hits, file_id);
    let root = cache_dir(db).await?;
    if let Some(path) = root.as_ref().and_then(|dir| existing(dir, file_id)) {
        let bytes = std::fs::read(&path).map_err(|_| fail(502, "附件下载失败"))?;
        let fallback = path
            .file_name()
            .and_then(|item| item.to_str())
            .unwrap_or("download")
            .to_string();
        return Ok(Downloaded {
            disposition: content_disposition(&name, &fallback),
            bytes,
        });
    }
    let remote = fresh_url(&hits, file_id).filter(|url| url_allowed(url, system_resolve));
    let remote = match remote {
        Some(url) => url,
        None => resolve_remote(db, file_id).await?,
    };
    if !url_allowed(&remote, system_resolve) {
        return Err(fail(502, "附件地址不安全"));
    }
    let Some(dir) = root else {
        return Err(fail(502, "附件下载失败"));
    };
    let _permit = slots()
        .acquire()
        .await
        .map_err(|_| fail(502, "附件下载失败"))?;
    let stored_id = file_id.to_string();
    let stored_name = name.clone();
    let saved =
        tokio::task::spawn_blocking(move || fetch_and_store(dir, stored_id, stored_name, remote))
            .await
            .map_err(|_| fail(502, "附件下载失败"))?
            .map_err(|detail| fail(502, detail))?;
    let local = format!("/zsxq-files/{}", saved.file_name);
    write_back(db, &hits, file_id, &local).await?;
    Ok(Downloaded {
        disposition: content_disposition(&name, &saved.file_name),
        bytes: saved.bytes,
    })
}

struct Stored {
    bytes: Vec<u8>,
    file_name: String,
}

fn fetch_and_store(
    dir: PathBuf,
    file_id: String,
    name: String,
    url: String,
) -> Result<Stored, &'static str> {
    std::fs::create_dir_all(&dir).map_err(|_| "附件下载失败")?;
    if let Some(path) = existing(&dir, &file_id) {
        let bytes = std::fs::read(&path).map_err(|_| "附件下载失败")?;
        let file_name = path
            .file_name()
            .and_then(|item| item.to_str())
            .unwrap_or("download")
            .to_string();
        return Ok(Stored { bytes, file_name });
    }
    let (bytes, content_type) = pull(&url)?;
    let ext = extension(&name, &content_type);
    let file_name = format!("{file_id}.{ext}");
    let target = dir.join(&file_name);
    let part = dir.join(format!("{file_name}.part"));
    let mut file = std::fs::File::create(&part).map_err(|_| "附件下载失败")?;
    file.write_all(&bytes).map_err(|_| "附件下载失败")?;
    drop(file);
    std::fs::rename(&part, &target).map_err(|_| "附件下载失败")?;
    Ok(Stored { bytes, file_name })
}

fn pull(start: &str) -> Result<(Vec<u8>, String), &'static str> {
    let agent = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .redirects(0)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build();
    let mut current = start.to_string();
    for _ in 0..4 {
        if !url_allowed(&current, system_resolve) {
            return Err("附件地址不安全");
        }
        match agent.get(&current).call() {
            Ok(response) => {
                let content_type = response.header("content-type").unwrap_or("").to_string();
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .take(MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "附件下载失败")?;
                if bytes.is_empty() || bytes.len() > MAX_BYTES {
                    return Err("附件下载失败");
                }
                return Ok((bytes, content_type));
            }
            Err(ureq::Error::Status(code, response)) if (300..400).contains(&code) => {
                let location = response.header("location").unwrap_or("").to_string();
                let _ = response.into_string();
                if !(location.starts_with("https://") || location.starts_with("http://")) {
                    return Err("附件地址不安全");
                }
                current = location;
            }
            Err(_) => return Err("附件下载失败"),
        }
    }
    Err("附件下载失败")
}

async fn resolve_remote(db: &Db, file_id: &str) -> Result<String, FileError> {
    let Some(token) = crate::zsxq::access_token(db)
        .await
        .map_err(|_| fail(502, "附件暂时无法下载"))?
    else {
        return Err(fail(502, "附件暂时无法下载"));
    };
    let file_id = file_id.to_string();
    tokio::task::spawn_blocking(move || request_download_url(&token, &file_id))
        .await
        .map_err(|_| fail(502, "附件暂时无法下载"))?
}

fn request_download_url(token: &str, file_id: &str) -> Result<String, FileError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(20))
        .build();
    let url = format!("{API}/{file_id}/download_url");
    for attempt in 0..6 {
        let text = match agent
            .get(&url)
            .set("Cookie", &format!("zsxq_access_token={token}"))
            .set("Origin", "https://wx.zsxq.com")
            .set("Referer", "https://wx.zsxq.com/")
            .call()
        {
            Ok(response) => response
                .into_string()
                .map_err(|_| fail(502, "附件暂时无法下载"))?,
            Err(ureq::Error::Status(_, response)) => response.into_string().unwrap_or_default(),
            Err(_) => return Err(fail(502, "附件暂时无法下载")),
        };
        match interpret_download(&text) {
            Download::Url(url) => return Ok(url),
            Download::Retry if attempt < 5 => continue,
            Download::Limited(detail) => return Err(fail(429, detail)),
            Download::Retry | Download::Empty => return Err(fail(502, "附件暂时无法下载")),
        }
    }
    Err(fail(502, "附件暂时无法下载"))
}

enum Download {
    Url(String),
    Retry,
    Limited(String),
    Empty,
}

fn interpret_download(text: &str) -> Download {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Download::Empty;
    };
    if value.get("succeeded") == Some(&json!(true)) {
        let url = value["resp_data"]["download_url"]
            .as_str()
            .unwrap_or("")
            .to_string();
        return if url.is_empty() {
            Download::Empty
        } else {
            Download::Url(url)
        };
    }
    match value.get("code").and_then(Value::as_i64) {
        Some(13607 | 20601) => Download::Limited(format!(
            "附件下载受限：知识星球下载受限 code={} {}",
            value["code"],
            value.get("info").and_then(Value::as_str).unwrap_or("")
        )),
        Some(1059) => Download::Retry,
        _ => Download::Empty,
    }
}

async fn cache_dir(db: &Db) -> Result<Option<PathBuf>, FileError> {
    let path = db
        .sqlite_path()
        .await
        .map_err(|_| fail(500, "读取附件失败"))?;
    if path.is_empty() || path == ":memory:" {
        return Ok(None);
    }
    Ok(Some(
        PathBuf::from(path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("zsxq_files"),
    ))
}

fn existing(dir: &std::path::Path, file_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut hits = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some((id, ext)) = name.split_once('.') else {
            continue;
        };
        if id == file_id
            && !ext.eq_ignore_ascii_case("part")
            && ext.len() <= 5
            && ext.chars().all(|ch| ch.is_ascii_alphanumeric())
            && entry.path().is_file()
        {
            hits.push(entry.path());
        }
    }
    hits.sort();
    hits.into_iter().next()
}

fn file_name(hits: &[(i64, i64, String)], file_id: &str) -> String {
    for (_, _, detail) in hits {
        let Ok(parsed) = serde_json::from_str::<Value>(detail) else {
            continue;
        };
        let Some(files) = parsed.get("files").and_then(Value::as_array) else {
            continue;
        };
        for file in files {
            if file_id_of(file) == file_id {
                let name = file
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    String::new()
}

fn fresh_url(hits: &[(i64, i64, String)], file_id: &str) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|item| item.as_secs())
        .unwrap_or(0);
    for (_, _, detail) in hits {
        let Ok(parsed) = serde_json::from_str::<Value>(detail) else {
            continue;
        };
        let Some(files) = parsed.get("files").and_then(Value::as_array) else {
            continue;
        };
        for file in files {
            if file_id_of(file) != file_id {
                continue;
            }
            let url = file.get("url").and_then(Value::as_str).unwrap_or("");
            if url.is_empty() {
                continue;
            }
            if expiry(url).is_none_or(|expires| expires > now) {
                return Some(url.to_string());
            }
        }
    }
    None
}

fn expiry(url: &str) -> Option<u64> {
    url.split(['?', '&'])
        .find_map(|part| part.strip_prefix("e="))
        .and_then(|value| value.parse().ok())
}

async fn write_back(
    db: &Db,
    hits: &[(i64, i64, String)],
    file_id: &str,
    local: &str,
) -> Result<(), FileError> {
    for (id, _, detail) in hits {
        let Ok(mut parsed) = serde_json::from_str::<Value>(detail) else {
            continue;
        };
        let Some(files) = parsed.get_mut("files").and_then(Value::as_array_mut) else {
            continue;
        };
        let mut changed = false;
        for file in files {
            if file_id_of(file) == file_id {
                file["url"] = json!(local);
                changed = true;
            }
        }
        if changed {
            db.replace_post_detail(*id, &parsed.to_string())
                .await
                .map_err(|_| fail(500, "读取附件失败"))?;
        }
    }
    Ok(())
}

fn file_id_of(file: &Value) -> String {
    match &file["file_id"] {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

fn extension(name: &str, content_type: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if !ext.is_empty()
        && ext.len() <= 5
        && ext.chars().all(|ch| ch.is_ascii_alphanumeric())
        && ext != name.to_ascii_lowercase()
    {
        return ext;
    }
    match content_type.split(';').next().unwrap_or("").trim() {
        "application/pdf" => "pdf".into(),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx".into(),
        _ => "bin".into(),
    }
}

pub fn content_disposition(name: &str, fallback: &str) -> String {
    let chosen = if name.is_empty() { fallback } else { name };
    let ascii: String = chosen
        .chars()
        .filter(|ch| ch.is_ascii() && *ch != '"' && *ch != '\\' && !ch.is_control())
        .collect();
    let ascii = if ascii.is_empty() {
        "download".into()
    } else {
        ascii
    };
    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        encode(chosen)
    )
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn url_allowed(url: &str, resolve: impl Fn(&str) -> Vec<IpAddr>) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    if let Ok(ip) = host.parse::<IpAddr>() {
        return !blocked(ip);
    }
    let ips = resolve(&host);
    !ips.is_empty() && ips.iter().all(|ip| !blocked(*ip))
}

fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    if rest.is_empty() || rest.contains('@') {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if let Some(host) = authority.strip_prefix('[') {
        return host
            .split(']')
            .next()
            .filter(|host| !host.is_empty())
            .map(|host| host.to_string());
    }
    authority
        .split(':')
        .next()
        .filter(|host| !host.is_empty())
        .map(|host| host.to_string())
}

fn system_resolve(host: &str) -> Vec<IpAddr> {
    (host, 0u16)
        .to_socket_addrs()
        .map(|items| items.map(|item| item.ip()).collect())
        .unwrap_or_default()
}

fn blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => blocked_v4(ip),
        IpAddr::V6(ip) => ip.to_ipv4_mapped().is_some_and(blocked_v4) || blocked_v6(ip),
    }
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 192 && b == 0 && ip.octets()[2] == 0)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 240
}

fn blocked_v6(ip: Ipv6Addr) -> bool {
    let [a, ..] = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (a & 0xfe00) == 0xfc00
        || (a & 0xffc0) == 0xfe80
        || a == 0x0064 && ip.segments()[1] == 0xff9b
}

fn slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(4))
}

pub async fn cache_stats(db: &Db) -> Value {
    let Ok(Some(dir)) = cache_dir(db).await else {
        return json!({"files": 0, "bytes": 0});
    };
    if !dir.is_dir() {
        return json!({"files": 0, "bytes": 0});
    }
    tokio::task::spawn_blocking(move || dir_usage(&dir))
        .await
        .unwrap_or_else(|_| json!({"files": 0, "bytes": 0}))
}

pub async fn purge(db: &Db) -> Result<Value, FileError> {
    let Some(dir) = cache_dir(db).await? else {
        return Ok(json!({"deleted": 0, "files": 0, "bytes": 0}));
    };
    if !dir.is_dir() {
        return Ok(json!({"deleted": 0, "files": 0, "bytes": 0}));
    }
    let keep = referenced_ids(db).await?;
    let deleted = tokio::task::spawn_blocking(move || delete_unreferenced(&dir, &keep))
        .await
        .map_err(|_| fail(500, "清理缓存失败"))?;
    let mut usage = cache_stats(db).await;
    usage["deleted"] = json!(deleted);
    Ok(usage)
}

async fn referenced_ids(db: &Db) -> Result<Vec<String>, FileError> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT detail FROM posts WHERE platform = 'zsxq' AND detail LIKE '%file_id%'",
    )
    .fetch_all(db.pool())
    .await
    .map_err(|_| fail(500, "清理缓存失败"))?;
    let mut keep = Vec::new();
    for raw in rows {
        let Ok(detail) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(files) = detail.get("files").and_then(Value::as_array) else {
            continue;
        };
        for file in files {
            if let Some(id) = referenced_id(file) {
                if !keep.iter().any(|item| item == &id) {
                    keep.push(id);
                }
            }
        }
    }
    Ok(keep)
}

fn referenced_id(file: &Value) -> Option<String> {
    match &file["file_id"] {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) if number.as_i64() != Some(0) => Some(number.to_string()),
        _ => None,
    }
}

fn dir_usage(dir: &std::path::Path) -> Value {
    let mut files = 0i64;
    let mut bytes = 0i64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return json!({"files": 0, "bytes": 0});
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() {
            files += 1;
            bytes += meta.len() as i64;
        }
    }
    json!({"files": files, "bytes": bytes})
}

fn delete_unreferenced(dir: &std::path::Path, keep: &[String]) -> i64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut deleted = 0i64;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = cached_id(&name) else { continue };
        if keep.iter().any(|item| item == id) {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

fn cached_id(name: &str) -> Option<&str> {
    let (id, ext) = name.split_once('.')?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if (1..=5).contains(&ext.len()) && ext.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Some(id)
    } else {
        None
    }
}

fn fail(status: u16, detail: impl Into<String>) -> FileError {
    FileError {
        status,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_expired_links() {
        assert!(!url_allowed("https://127.0.0.1/a", |_| Vec::new()));
        assert!(!url_allowed(
            "https://169.254.169.254/latest",
            |_| Vec::new()
        ));
        assert!(!url_allowed("https://user:pass@1.1.1.1/a", |_| Vec::new()));
        assert!(url_allowed("https://1.1.1.1/a", |_| Vec::new()));
        assert!(!url_allowed("https://files.example/a", |_| vec![
            IpAddr::V4(Ipv4Addr::new(10, 1, 1, 1))
        ]));
        assert!(matches!(
            interpret_download(r#"{"succeeded":false,"code":13607,"info":"日限"}"#),
            Download::Limited(_)
        ));
        assert!(matches!(
            interpret_download(
                r#"{"succeeded":true,"resp_data":{"download_url":"https://1.1.1.1/a"}}"#
            ),
            Download::Url(_)
        ));
        let hits = vec![(
            1,
            2,
            r#"{"files":[{"file_id":"9","name":"纪要.pdf","url":"https://1.1.1.1/a?e=1"}]}"#.into(),
        )];
        assert!(fresh_url(&hits, "9").is_none());
    }

    #[tokio::test]
    async fn serves_cached_file_only_to_subscribers() {
        let path = std::env::temp_dir().join(format!(
            "vpush-zsxq-file-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('reader', 'x')")
            .execute(db.pool())
            .await
            .unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'reader'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let kol = db
            .add_kol("zsxq", "星球", "100", None, false, false, false)
            .await
            .unwrap();
        db.save_fetched(
            kol,
            "topic",
            "标题",
            "正文",
            "post",
            "[]",
            "https://wx.zsxq.com/group/100/topic",
            "2026-07-01 00:00",
        )
        .await
        .unwrap();
        db.set_platform_detail(
            "zsxq",
            "topic",
            r#"{"files":[{"file_id":"42","name":"纪要.pdf"},{"file_id":"420","name":"别的.pdf"}]}"#,
        )
        .await
        .unwrap();
        let dir = path.parent().unwrap().join("zsxq_files");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("42.pdf"), b"pdf-bytes").unwrap();
        assert!(matches!(
            read_file(&db, user, false, "42").await,
            Err(FileError { status: 404, .. })
        ));
        db.subscribe(user, false, kol, "post").await.unwrap();
        let got = read_file(&db, user, false, "42").await.unwrap();
        assert_eq!(got.bytes, b"pdf-bytes");
        assert!(got.disposition.contains("filename*=UTF-8''"));
        assert!(matches!(
            read_file(&db, user, false, "7").await,
            Err(FileError { status: 404, .. })
        ));
        assert!(matches!(
            read_file(&db, user, false, "abc").await,
            Err(FileError { status: 400, .. })
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(dir.join("42.pdf"));
    }

    #[tokio::test]
    async fn purge_removes_only_unreferenced_files() {
        let path = std::env::temp_dir().join(format!(
            "vpush-zsxq-purge-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let kol = db
            .add_kol("zsxq", "g", "123", None, false, false, false)
            .await
            .unwrap();
        db.save_fetched(kol, "t1", "t", "c", "post", "[]", "u", "2026-08-20")
            .await
            .unwrap();
        db.set_platform_detail(
            "zsxq",
            "t1",
            r#"{"files":[{"file_id":"22","name":"a.pdf"}]}"#,
        )
        .await
        .unwrap();
        let dir = path.parent().unwrap().join("zsxq_files");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("22.pdf"), b"keep").unwrap();
        std::fs::write(dir.join("99.pdf"), b"xxxx").unwrap();
        std::fs::write(dir.join("notes.txt"), b"stay").unwrap();
        let before = cache_stats(&db).await;
        assert_eq!(before["files"], 3);
        assert_eq!(before["bytes"], 12);
        let purged = purge(&db).await.unwrap();
        assert_eq!(purged["deleted"], 1);
        assert_eq!(purged["files"], 2);
        assert!(dir.join("22.pdf").is_file());
        assert!(dir.join("notes.txt").is_file());
        assert!(!dir.join("99.pdf").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&path);
    }
}
