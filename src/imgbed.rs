use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::db::Db;
use crate::img_proxy;

const MAX_RETENTION: i64 = 3650;

pub struct Input<'a> {
    pub base_url: &'a str,
    pub token: &'a str,
    pub channel_name: &'a str,
    pub folder: &'a str,
    pub retention_days: Option<i64>,
}

#[derive(Debug)]
pub struct ImgbedError {
    pub status: u16,
    pub detail: &'static str,
}

pub async fn status(db: &Db) -> Result<Value, sqlx::Error> {
    let stored_url = text(db, "imgbed_base_url").await?;
    let stored_token = text(db, "imgbed_token").await?;
    let env_url = env_text("IMGBED_BASE_URL");
    let env_token = env_text("IMGBED_TOKEN");
    let base_url = nonempty(&stored_url).unwrap_or(env_url);
    let token_set = !stored_token.is_empty() || !env_token.is_empty();
    let (ready, pending, failed) = db.hosted_image_counts().await?;
    Ok(json!({
        "project": "CloudFlare-ImgBed",
        "project_url": "https://github.com/MarSeventh/CloudFlare-ImgBed",
        "base_url": base_url,
        "token_set": token_set,
        "token_from_env": !env_token.is_empty() && stored_token.is_empty(),
        "updated_at": text(db, "imgbed_updated_at").await?,
        "channel": fallback(&text(db, "imgbed_channel").await?, &env_text("IMGBED_CHANNEL"), "telegram"),
        "channel_name": fallback(&text(db, "imgbed_channel_name").await?, &env_text("IMGBED_CHANNEL_NAME"), "vpush-imgbed"),
        "folder": fallback(&text(db, "imgbed_folder").await?, &env_text("IMGBED_FOLDER"), "vpush"),
        "enabled": !base_url.is_empty() && token_set,
        "ready_count": ready,
        "pending_count": pending,
        "failed_count": failed,
        "last_check_error": text(db, "imgbed_last_check_error").await?,
        "retention_days": retention(&text(db, "imgbed_retention_days").await?),
    }))
}

pub async fn save(
    db: &Db,
    input: Input<'_>,
    probe: fn(&str) -> String,
) -> Result<Value, ImgbedError> {
    let current = status(db)
        .await
        .map_err(|_| fail(500, "保存图床设置失败"))?;
    let base_url = if input.base_url.trim().is_empty() {
        current["base_url"].as_str().unwrap_or("").to_string()
    } else {
        normalize_base(input.base_url)?
    };
    let token = if input.token.trim().is_empty() {
        let stored = text(db, "imgbed_token")
            .await
            .map_err(|_| fail(500, "保存图床设置失败"))?;
        if !stored.is_empty() {
            stored
        } else {
            env_text("IMGBED_TOKEN")
        }
    } else {
        clean_token(input.token)?
    };
    if token.is_empty() {
        return Err(fail(400, "请填写 API 密钥"));
    }
    let channel_name = label(
        input.channel_name,
        current["channel_name"].as_str().unwrap_or("vpush-imgbed"),
        "vpush-imgbed",
    )?;
    let folder = label(
        input.folder.trim().trim_matches('/'),
        current["folder"].as_str().unwrap_or("vpush"),
        "vpush",
    )?;
    if folder.contains("..") {
        return Err(fail(400, "目录无效"));
    }
    let channel = current["channel"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or("telegram")
        .to_string();
    if let Some(days) = input.retention_days {
        if !(0..=MAX_RETENTION).contains(&days) {
            return Err(fail(400, "图片保留天数须在 0–3650"));
        }
        db.set_setting("imgbed_retention_days", &days.to_string())
            .await
            .map_err(|_| fail(500, "保存图床设置失败"))?;
    }
    let check = if base_url.is_empty() {
        "未配置图床地址".to_string()
    } else {
        let url = base_url.clone();
        tokio::task::spawn_blocking(move || probe(&url))
            .await
            .unwrap_or_else(|_| "连通检查失败".into())
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for (key, value) in [
        ("imgbed_base_url", base_url.as_str()),
        ("imgbed_token", token.as_str()),
        ("imgbed_channel", channel.as_str()),
        ("imgbed_channel_name", channel_name.as_str()),
        ("imgbed_folder", folder.as_str()),
        ("imgbed_updated_at", &now.to_string()),
        ("imgbed_last_check_error", check.as_str()),
    ] {
        db.set_setting(key, value)
            .await
            .map_err(|_| fail(500, "保存图床设置失败"))?;
    }
    status(db).await.map_err(|_| fail(500, "保存图床设置失败"))
}

pub async fn clear(db: &Db) -> Result<Value, sqlx::Error> {
    for key in [
        "imgbed_base_url",
        "imgbed_token",
        "imgbed_channel",
        "imgbed_channel_name",
        "imgbed_folder",
        "imgbed_updated_at",
        "imgbed_last_check_error",
    ] {
        db.set_setting(key, "").await?;
    }
    status(db).await
}

pub fn normalize_base(raw: &str) -> Result<String, ImgbedError> {
    let raw = raw.trim().trim_end_matches('/');
    if !raw.to_ascii_lowercase().starts_with("https://") || raw[8..].contains('@') {
        return Err(fail(400, "图床地址须为 https 域名，不要带账号密码"));
    }
    let authority = raw[8..].split(['/', '?', '#']).next().unwrap_or("");
    let (host, port) =
        split_host(authority).ok_or(fail(400, "图床地址须为 https 域名，不要带账号密码"))?;
    if !host_ok(&host) {
        return Err(fail(400, "图床地址不可用"));
    }
    match port {
        Some(port) => Ok(format!("https://{host}:{port}")),
        None => Ok(format!("https://{host}")),
    }
}

pub fn probe(base_url: &str) -> String {
    let host = base_url
        .trim_start_matches("https://")
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    if host
        .parse::<IpAddr>()
        .map(|ip| !img_proxy::ip_allowed(ip))
        .unwrap_or(false)
        || !public_name(host)
    {
        return "图床地址不可用".into();
    }
    let ips = img_proxy::resolve(host);
    if !img_proxy::resolution_ok(&ips) {
        return "图床地址不可用".into();
    }
    let agent = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(5))
        .redirects(0)
        .build();
    match agent.get(&format!("{base_url}/")).call() {
        Ok(resp) if resp.status() < 500 => String::new(),
        Ok(resp) => format!("图床返回 HTTP {}", resp.status()),
        Err(ureq::Error::Status(code, _)) if code < 500 => String::new(),
        Err(ureq::Error::Status(code, _)) => format!("图床返回 HTTP {code}"),
        Err(err) => err.to_string().chars().take(180).collect(),
    }
}

fn host_ok(host: &str) -> bool {
    if !public_name(host) {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => img_proxy::ip_allowed(ip),
        Err(_) => true,
    }
}

fn public_name(host: &str) -> bool {
    let host = host
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    !host.is_empty()
        && host != "localhost"
        && !host.ends_with(".local")
        && host != "metadata.google.internal"
}

fn split_host(authority: &str) -> Option<(String, Option<u16>)> {
    let (host, port) = if let Some(host) = authority.strip_prefix('[') {
        let (host, rest) = host.split_once(']')?;
        let port = rest.strip_prefix(':').unwrap_or("");
        (host.to_ascii_lowercase(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
                (host.to_ascii_lowercase(), port)
            }
            _ => (authority.to_ascii_lowercase(), ""),
        }
    };
    if host.is_empty() || host.contains(':') {
        return None;
    }
    let port = if port.is_empty() {
        None
    } else {
        Some(port.parse().ok()?)
    };
    if port.is_some_and(|port| port == 0) {
        return None;
    }
    Some((host, port))
}

fn clean_token(raw: &str) -> Result<String, ImgbedError> {
    let token = raw.trim();
    if token.is_empty() || token.len() > 512 || token.chars().any(|c| c.is_control()) {
        return Err(fail(400, "API 密钥无效"));
    }
    Ok(token.to_string())
}

fn label(raw: &str, current: &str, default: &str) -> Result<String, ImgbedError> {
    let value = raw.trim();
    let value = if value.is_empty() {
        if current.is_empty() {
            default
        } else {
            current
        }
    } else {
        value
    };
    if value.len() > 64
        || value
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(fail(400, "图床渠道或目录无效"));
    }
    Ok(value.to_string())
}

fn retention(raw: &str) -> i64 {
    raw.parse::<i64>()
        .ok()
        .filter(|days| (0..=MAX_RETENTION).contains(days))
        .unwrap_or(30)
}

fn fallback(stored: &str, env: &str, default: &str) -> String {
    nonempty(stored)
        .or_else(|| nonempty(env))
        .unwrap_or_else(|| default.to_string())
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn env_text(key: &str) -> String {
    std::env::var(key).unwrap_or_default().trim().to_string()
}

async fn text(db: &Db, key: &str) -> Result<String, sqlx::Error> {
    Ok(db.setting(key).await?.unwrap_or_default())
}

fn fail(status: u16, detail: &'static str) -> ImgbedError {
    ImgbedError { status, detail }
}

#[derive(Clone)]
pub struct Runtime {
    pub base_url: String,
    pub token: String,
    pub channel: String,
    pub channel_name: String,
    pub folder: String,
    pub retention_days: i64,
}

pub async fn runtime(db: &Db) -> Option<Runtime> {
    let current = status(db).await.ok()?;
    if current["enabled"] != true {
        return None;
    }
    let token = text(db, "imgbed_token")
        .await
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(env_token);
    if token.is_empty() {
        return None;
    }
    Some(Runtime {
        base_url: current["base_url"]
            .as_str()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string(),
        token,
        channel: current["channel"]
            .as_str()
            .unwrap_or("telegram")
            .to_string(),
        channel_name: current["channel_name"]
            .as_str()
            .unwrap_or("vpush-imgbed")
            .to_string(),
        folder: current["folder"].as_str().unwrap_or("vpush").to_string(),
        retention_days: current["retention_days"].as_i64().unwrap_or(30),
    })
}

fn env_token() -> String {
    env_text("IMGBED_TOKEN")
}

pub async fn process_due<D, U, R>(
    db: &Db,
    retention_days: i64,
    mut download: D,
    mut upload: U,
    mut remove: R,
) -> Result<usize, sqlx::Error>
where
    D: FnMut(&str) -> Result<(Vec<u8>, String), String>,
    U: FnMut(&str, &[u8], &str) -> Result<String, String>,
    R: FnMut(&str) -> bool,
{
    use sha2::{Digest, Sha256};
    use sqlx::Row;
    if retention_days > 0 {
        let expired = sqlx::query(
            "SELECT source_url, hosted_url FROM hosted_images
             WHERE status = 'ready' AND hosted_url != '' AND created_at <= datetime('now', ?)",
        )
        .bind(format!("-{retention_days} days"))
        .fetch_all(db.pool())
        .await?;
        for row in expired {
            let hosted: String = row.get("hosted_url");
            if remove(hosted.as_str()) {
                sqlx::query("DELETE FROM hosted_images WHERE source_url = ?")
                    .bind(row.get::<String, _>("source_url"))
                    .execute(db.pool())
                    .await?;
            }
        }
    }
    let due = sqlx::query(
        "SELECT source_url FROM hosted_images
         WHERE status IN ('pending', 'failed') AND attempts < 5
         AND (attempts = 0 OR last_attempt_at = '' OR last_attempt_at <= datetime('now', '-15 minutes'))
         ORDER BY created_at LIMIT 10",
    )
    .fetch_all(db.pool())
    .await?;
    let mut ready = 0;
    for row in due {
        let source: String = row.get("source_url");
        let (bytes, kind) = match download(&source) {
            Ok(item) => item,
            Err(err) => {
                mark_failed(db, &source, &err).await?;
                continue;
            }
        };
        let digest = hex::encode(Sha256::digest(&bytes));
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT hosted_url FROM hosted_images WHERE content_hash = ? AND status = 'ready' AND hosted_url != '' LIMIT 1",
        )
        .bind(&digest)
        .fetch_optional(db.pool())
        .await?
        {
            mark_ready(db, &source, &existing, &digest).await?;
            ready += 1;
            continue;
        }
        match upload(&source, &bytes, &kind) {
            Ok(hosted) => {
                mark_ready(db, &source, &hosted, &digest).await?;
                ready += 1;
            }
            Err(err) => mark_failed(db, &source, &err).await?,
        }
    }
    Ok(ready)
}

async fn mark_ready(db: &Db, source: &str, hosted: &str, digest: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE hosted_images SET status = 'ready', hosted_url = ?, content_hash = ?, attempts = attempts + 1, last_error = '', last_attempt_at = datetime('now') WHERE source_url = ?",
    )
    .bind(hosted)
    .bind(digest)
    .bind(source)
    .execute(db.pool())
    .await?;
    Ok(())
}

async fn mark_failed(db: &Db, source: &str, error: &str) -> Result<(), sqlx::Error> {
    let error: String = error.chars().take(300).collect();
    sqlx::query(
        "UPDATE hosted_images SET status = 'failed', attempts = attempts + 1, last_error = ?, last_attempt_at = datetime('now') WHERE source_url = ?",
    )
    .bind(error)
    .bind(source)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub fn live_download(url: &str) -> Result<(Vec<u8>, String), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("源图下载失败".into());
    }
    let response = if url_host(url) == "static-assets-1.truthsocial.com" {
        if let Some(origin) = truth_origin() {
            origin_response(url, origin)?
        } else {
            direct_response(url)?
        }
    } else {
        direct_response(url)?
    };
    let kind = response
        .content_type()
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let max = if kind == "video/mp4" {
        50_000_000
    } else {
        8_000_000
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(max)
        .read_to_end(&mut bytes)
        .map_err(|_| "源图下载失败".to_string())?;
    if !matches!(
        kind.as_str(),
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "video/mp4"
    ) {
        return Err(format!("非图片内容 {kind}"));
    }
    if bytes.len() <= 2048 {
        return Err("图片过小".into());
    }
    Ok((bytes, kind))
}

pub fn live_upload(
    cfg: &Runtime,
    source_url: &str,
    bytes: &[u8],
    kind: &str,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let ext = match kind {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        _ => return Err("非图片内容".into()),
    };
    let name = format!(
        "{}.{ext}",
        &hex::encode(Sha256::digest(source_url.as_bytes()))[..16]
    );
    let boundary = "vpushimgbedboundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {kind}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let url = format!(
        "{}/upload?uploadChannel={}&returnFormat=full&uploadNameType=index&serverCompress=false&channelName={}&uploadFolder={}",
        cfg.base_url,
        urlencoding(&cfg.channel),
        urlencoding(&cfg.channel_name),
        urlencoding(&cfg.folder)
    );
    let response = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout(std::time::Duration::from_secs(30))
        .redirects(0)
        .build()
        .post(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .set("Origin", &cfg.base_url)
        .send_bytes(&body)
        .map_err(|err| match err {
            ureq::Error::Status(code, _) => format!("图床上传失败 HTTP {code}"),
            _ => "图床上传失败".into(),
        })?;
    let text = response
        .into_string()
        .map_err(|_| "图床上传响应不是 JSON".to_string())?;
    let payload: Value =
        serde_json::from_str(&text).map_err(|_| "图床上传响应不是 JSON".to_string())?;
    let hosted = hosted_url(&payload, &cfg.base_url);
    if hosted.is_empty() {
        return Err("图床上传未返回公开地址".into());
    }
    if !hosted.starts_with(&format!("{}/", cfg.base_url)) {
        return Err("图床返回了非本域地址".into());
    }
    Ok(hosted)
}

fn hosted_url(payload: &Value, base_url: &str) -> String {
    let items = payload
        .as_array()
        .map(|items| items.as_slice())
        .unwrap_or(std::slice::from_ref(payload));
    for item in items {
        for key in ["publicUrl", "src"] {
            let raw = item
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            if raw.is_empty() {
                continue;
            }
            if let Some(path) = raw.strip_prefix('/') {
                return format!("{base_url}/{path}");
            }
            return raw.to_string();
        }
    }
    String::new()
}

pub fn live_delete(cfg: &Runtime, hosted_url: &str) -> bool {
    let prefix = format!("{}/file/", cfg.base_url);
    let Some(path) = hosted_url.strip_prefix(&prefix) else {
        return true;
    };
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return true;
    }
    let url = format!(
        "{}/api/manage/delete/{}",
        cfg.base_url,
        urlencoding_path(path)
    );
    match ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout(std::time::Duration::from_secs(15))
        .redirects(0)
        .build()
        .get(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(_) => true,
        Err(ureq::Error::Status(code, _)) if code == 404 || code == 410 => true,
        _ => false,
    }
}

fn urlencoding(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn urlencoding_path(value: &str) -> String {
    value
        .split('/')
        .map(urlencoding)
        .collect::<Vec<_>>()
        .join("/")
}

fn truth_origin() -> Option<IpAddr> {
    std::env::var("TRUTH_IMAGE_ORIGIN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse().ok())
}

fn truth_fetch_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = path
        .trim_start_matches("https://static-assets-1.truthsocial.com/")
        .trim_start_matches("http://static-assets-1.truthsocial.com/");
    format!("https://vpush.net/internal/img-fetch/{path}")
}

fn origin_response(url: &str, origin: IpAddr) -> Result<ureq::Response, String> {
    // ponytail: ARM's address is challenged by Truth's CDN; DMIT is not
    ureq::AgentBuilder::new()
        .resolver(move |netloc: &str| {
            let Some((host, port)) = netloc.rsplit_once(':') else {
                return crate::url_guard::public_resolver(netloc);
            };
            if host == "vpush.net" {
                let port: u16 = port.parse().unwrap_or(443);
                return Ok(vec![SocketAddr::new(origin, port)]);
            }
            crate::url_guard::public_resolver(netloc)
        })
        .timeout(Duration::from_secs(20))
        .redirects(0)
        .build()
        .get(&truth_fetch_url(url))
        .set("User-Agent", "Mozilla/5.0")
        .call()
        .map_err(|err| match err {
            ureq::Error::Status(code, _) => format!("源图下载失败 HTTP {code}"),
            _ => "源图下载失败".into(),
        })
}

fn direct_response(url: &str) -> Result<ureq::Response, String> {
    ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build()
        .get(url)
        .set("User-Agent", "Mozilla/5.0")
        .call()
        .map_err(|err| match err {
            ureq::Error::Status(code, _) => format!("源图下载失败 HTTP {code}"),
            _ => "源图下载失败".into(),
        })
}

pub async fn enqueue_images(db: &Db, images_json: &str) -> Result<(), sqlx::Error> {
    if !configured(db).await? {
        return Ok(());
    }
    let Ok(urls) = serde_json::from_str::<Vec<Value>>(images_json) else {
        return Ok(());
    };
    for url in urls {
        let Some(url) = url.as_str().map(str::trim).filter(|url| mirror_source(url)) else {
            continue;
        };
        sqlx::query(
            "INSERT OR IGNORE INTO hosted_images (source_url, status) VALUES (?, 'pending')",
        )
        .bind(url)
        .execute(db.pool())
        .await?;
    }
    Ok(())
}

async fn configured(db: &Db) -> Result<bool, sqlx::Error> {
    let stored_url = text(db, "imgbed_base_url").await?;
    let base = nonempty(&stored_url).unwrap_or_else(|| env_text("IMGBED_BASE_URL"));
    let token = !text(db, "imgbed_token").await?.is_empty() || !env_text("IMGBED_TOKEN").is_empty();
    Ok(!base.is_empty() && token)
}

pub async fn rewrite_posts(db: &Db, posts: &mut [Value]) -> Result<(), sqlx::Error> {
    let host = base_host(db).await?;
    if host.is_empty() {
        return Ok(());
    }
    let mut urls = Vec::new();
    for post in posts.iter() {
        let Some(images) = post.get("images").and_then(Value::as_array) else {
            continue;
        };
        for image in images {
            let Some(url) = image.as_str().map(str::trim) else {
                continue;
            };
            if mirror_source(url) && !urls.iter().any(|seen: &String| seen == url) {
                urls.push(url.to_string());
            }
        }
    }
    if urls.is_empty() {
        return Ok(());
    }
    use sqlx::Row;
    let mut hosted = std::collections::HashMap::new();
    for chunk in urls.chunks(200) {
        let mut sql = String::from(
            "SELECT source_url, hosted_url FROM hosted_images WHERE status = 'ready' AND hosted_url != '' AND source_url IN (",
        );
        for (i, _) in chunk.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
        }
        sql.push(')');
        let mut query = sqlx::query(&sql);
        for url in chunk {
            query = query.bind(url);
        }
        for row in query.fetch_all(db.pool()).await? {
            let source: String = row.get("source_url");
            let url: String = row.get("hosted_url");
            if url_host(&url) == host && url.starts_with("https://") {
                hosted.insert(source, url);
            }
        }
    }
    for post in posts.iter_mut() {
        let Some(images) = post.get_mut("images").and_then(Value::as_array_mut) else {
            continue;
        };
        for image in images {
            let raw = image.as_str().unwrap_or("").trim().to_string();
            if let Some(url) = hosted.get(&raw) {
                *image = Value::String(url.clone());
            }
        }
    }
    Ok(())
}

async fn base_host(db: &Db) -> Result<String, sqlx::Error> {
    let stored = text(db, "imgbed_base_url").await?;
    Ok(url_host(
        &nonempty(&stored).unwrap_or_else(|| env_text("IMGBED_BASE_URL")),
    ))
}

fn mirror_source(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if path.ends_with(".mp4") || path.ends_with(".webm") {
        return false;
    }
    matches!(
        url_host(url).as_str(),
        "pbs.twimg.com" | "video.twimg.com" | "abs.twimg.com" | "static-assets-1.truthsocial.com"
    )
}

fn url_host(url: &str) -> String {
    let Some(rest) = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))
    else {
        return String::new();
    };
    if rest.contains('@') {
        return String::new();
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map(|(host, _)| host)
        .unwrap_or(authority);
    host.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(_: &str) -> String {
        String::new()
    }

    fn down(_: &str) -> String {
        "图床返回 HTTP 503".into()
    }

    #[test]
    fn address_must_be_public_https() {
        assert!(normalize_base("http://img.example.com").is_err());
        assert!(normalize_base("https://user:pw@img.example.com").is_err());
        assert!(normalize_base("https://127.0.0.1").is_err());
        assert!(normalize_base("https://10.1.2.3").is_err());
        assert!(normalize_base("https://169.254.169.254").is_err());
        assert!(normalize_base("https://localhost").is_err());
        assert_eq!(
            normalize_base("https://img.example.com/path").unwrap(),
            "https://img.example.com"
        );
    }

    #[tokio::test]
    async fn save_keeps_token_hidden_and_records_probe() {
        let path = std::env::temp_dir().join(format!(
            "vpush-imgbed-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let input = Input {
            base_url: "https://img.example.com",
            token: "",
            channel_name: "",
            folder: "",
            retention_days: Some(30),
        };
        assert_eq!(
            save(&db, input, quiet).await.unwrap_err().detail,
            "请填写 API 密钥"
        );
        let input = Input {
            base_url: "https://img.example.com/app",
            token: "secret-token",
            channel_name: "desk",
            folder: "vpush",
            retention_days: Some(7),
        };
        let saved = save(&db, input, down).await.unwrap();
        let body = saved.to_string();
        assert!(!body.contains("secret-token"));
        assert_eq!(saved["enabled"], true);
        assert_eq!(saved["base_url"], "https://img.example.com");
        assert_eq!(saved["token_set"], true);
        assert_eq!(saved["last_check_error"], "图床返回 HTTP 503");
        assert_eq!(saved["retention_days"], 7);
        assert_eq!(saved["channel_name"], "desk");
        let kept = save(
            &db,
            Input {
                base_url: "",
                token: "",
                channel_name: "",
                folder: "",
                retention_days: None,
            },
            quiet,
        )
        .await
        .unwrap();
        assert_eq!(kept["token_set"], true);
        assert_eq!(kept["last_check_error"], "");
        assert!(save(
            &db,
            Input {
                base_url: "https://img.example.com",
                token: "x",
                channel_name: "",
                folder: "",
                retention_days: Some(3651)
            },
            quiet
        )
        .await
        .is_err());
        db.note_hosted_image("https://pbs.twimg.com/a.jpg", "failed")
            .await
            .unwrap();
        let cleared = clear(&db).await.unwrap();
        assert_eq!(cleared["enabled"], false);
        assert_eq!(cleared["failed_count"], 1);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn mirror_reuses_hash_retries_when_due_and_purges_expired() {
        let path = std::env::temp_dir().join(format!(
            "vpush-mirror-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query(
            "INSERT INTO hosted_images (source_url, status, content_hash, hosted_url, created_at) VALUES
             ('https://cdn.example/new.jpg', 'pending', '', '', datetime('now')),
             ('https://cdn.example/same.jpg', 'pending', '', '', datetime('now')),
             ('https://cdn.example/soon.jpg', 'failed', '', '', datetime('now')),
             ('https://cdn.example/old.jpg', 'ready', 'abc', 'https://img.example/file/old.jpg', datetime('now', '-2 days'))",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE hosted_images SET attempts = 1, last_attempt_at = datetime('now') WHERE source_url = 'https://cdn.example/soon.jpg'")
            .execute(db.pool())
            .await
            .unwrap();
        let bytes = vec![9u8; 3000];
        let mut uploads = 0;
        let mut removed = 0;
        let ready = process_due(
            &db,
            1,
            |url| {
                if url.ends_with("same.jpg") || url.ends_with("new.jpg") {
                    Ok((bytes.clone(), "image/jpeg".into()))
                } else {
                    Err("超时".into())
                }
            },
            |url, _, _| {
                uploads += 1;
                Ok(format!("https://img.example/file/{url}"))
            },
            |_| {
                removed += 1;
                true
            },
        )
        .await
        .unwrap();
        assert_eq!(ready, 2);
        assert_eq!(uploads, 1);
        assert_eq!(removed, 1);
        let reused: String = sqlx::query_scalar("SELECT hosted_url FROM hosted_images WHERE source_url = 'https://cdn.example/same.jpg'").fetch_one(db.pool()).await.unwrap();
        assert!(reused.contains("new.jpg") || reused.contains("same.jpg"));
        let waiting: i64 = sqlx::query_scalar(
            "SELECT attempts FROM hosted_images WHERE source_url = 'https://cdn.example/soon.jpg'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(waiting, 1);
        let gone: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hosted_images WHERE source_url = 'https://cdn.example/old.jpg'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(gone, 0);
        let kept = process_due(
            &db,
            1,
            |_| Err("远端拒绝".into()),
            |_, _, _| Ok(String::new()),
            |_| false,
        )
        .await
        .unwrap();
        assert_eq!(kept, 0);
        let error: String = sqlx::query_scalar("SELECT last_error FROM hosted_images WHERE source_url = 'https://cdn.example/soon.jpg'").fetch_one(db.pool()).await.unwrap();
        assert!(error.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn rewrite_swaps_ready_mirror_only() {
        let path = std::env::temp_dir().join(format!(
            "vpush-rewrite-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.set_setting("imgbed_base_url", "https://img.example.com")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO hosted_images (source_url, status, hosted_url) VALUES
             ('https://pbs.twimg.com/a.jpg', 'ready', 'https://img.example.com/file/a.jpg'),
             ('https://pbs.twimg.com/b.jpg', 'pending', ''),
             ('https://video.twimg.com/v.mp4', 'ready', 'https://img.example.com/file/v.mp4'),
             ('https://pbs.twimg.com/evil.jpg', 'ready', 'https://evil.example/a.jpg')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let mut posts = vec![json!({
            "images": [
                "https://pbs.twimg.com/a.jpg",
                "https://pbs.twimg.com/b.jpg",
                "https://video.twimg.com/v.mp4",
                "https://pbs.twimg.com/evil.jpg",
                "https://img.example/a.jpg"
            ]
        })];
        rewrite_posts(&db, &mut posts).await.unwrap();
        let images = posts[0]["images"].as_array().unwrap();
        assert_eq!(images[0], "https://img.example.com/file/a.jpg");
        assert_eq!(images[1], "https://pbs.twimg.com/b.jpg");
        assert_eq!(images[2], "https://video.twimg.com/v.mp4");
        assert_eq!(images[3], "https://pbs.twimg.com/evil.jpg");
        assert_eq!(images[4], "https://img.example/a.jpg");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn save_fetched_queues_mirrorable_images() {
        let path = std::env::temp_dir().join(format!(
            "vpush-enqueue-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.set_setting("imgbed_base_url", "https://img.example.com")
            .await
            .unwrap();
        db.set_setting("imgbed_token", "secret").await.unwrap();
        let kol = db
            .add_kol("twitter", "甲", "111", None, false, false, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO hosted_images (source_url, status, hosted_url) VALUES ('https://pbs.twimg.com/a.jpg', 'ready', 'https://img.example.com/a.jpg')")
            .execute(db.pool())
            .await
            .unwrap();
        let images = r#"["https://pbs.twimg.com/a.jpg","https://pbs.twimg.com/b.jpg","https://video.twimg.com/v.mp4","https://img.example/a.jpg"]"#;
        db.save_fetched(kol, "p1", "", "正文", "post", images, "", "2026-09-28")
            .await
            .unwrap();
        let ready: String = sqlx::query_scalar(
            "SELECT status FROM hosted_images WHERE source_url = 'https://pbs.twimg.com/a.jpg'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let pending: String = sqlx::query_scalar(
            "SELECT status FROM hosted_images WHERE source_url = 'https://pbs.twimg.com/b.jpg'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hosted_images")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(ready, "ready");
        assert_eq!(pending, "pending");
        assert_eq!(rows, 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truth_fetch_url_points_at_the_origin_gate() {
        let url = "https://static-assets-1.truthsocial.com/tmtg:prime-ts-assets/a.jpg";
        let fetch = truth_fetch_url(url);
        assert_eq!(
            fetch,
            "https://vpush.net/internal/img-fetch/tmtg:prime-ts-assets/a.jpg"
        );
    }
}
