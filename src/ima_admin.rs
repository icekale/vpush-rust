use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::db::Db;

const MARKER: &str = ".vpush-local-library.json";
const SETTING: &str = "ima_local_libraries";

#[derive(Debug)]
pub struct AdminError {
    pub status: u16,
    pub detail: String,
}

pub async fn credentials(db: &Db) -> Result<Value, sqlx::Error> {
    let cookie = saved(db, "ima_cookie").await?;
    let client_id = saved(db, "ima_openapi_clientid").await?;
    let api_key = saved(db, "ima_openapi_apikey").await?;
    let cookie_value = if cookie.is_empty() {
        env_text("IMA_COOKIE")
    } else {
        cookie.clone()
    };
    let client_value = if client_id.is_empty() {
        env_text("IMA_OPENAPI_CLIENTID")
    } else {
        client_id.clone()
    };
    let key_value = if api_key.is_empty() {
        env_text("IMA_OPENAPI_APIKEY")
    } else {
        api_key
    };
    let mode = if !client_value.is_empty() && !key_value.is_empty() {
        "openapi"
    } else if !cookie_value.is_empty() {
        "cookie"
    } else {
        "none"
    };
    Ok(json!({
        "cookie": {
            "set": !cookie_value.is_empty(),
            "updated_at": if cookie.is_empty() { String::new() } else { saved(db, "ima_cookie_updated_at").await? },
            "preview": if cookie_value.is_empty() { "" } else { "已配置" },
            "from_env": !cookie_value.is_empty() && cookie.is_empty(),
        },
        "openapi_clientid": {"set": !client_value.is_empty(), "preview": preview(&client_value)},
        "openapi_apikey": {"set": !key_value.is_empty()},
        "mode": mode,
    }))
}

pub async fn save_credentials(
    db: &Db,
    cookie: &str,
    client_id: &str,
    api_key: &str,
) -> Result<(), AdminError> {
    let cookie = clean(cookie, 8192, "Cookie 无效")?;
    let client_id = clean(client_id, 256, "OpenAPI clientid 无效")?;
    let api_key = clean(api_key, 256, "OpenAPI apikey 无效")?;
    if cookie.is_empty() && (client_id.is_empty() || api_key.is_empty()) {
        return Err(bad(
            "需至少提供 ima Cookie 或 OpenAPI 凭证（clientid + apikey）",
        ));
    }
    if client_id.is_empty() != api_key.is_empty() {
        return Err(bad("OpenAPI 凭证需同时提供 clientid 与 apikey"));
    }
    if !cookie.is_empty() {
        db.set_setting("ima_cookie", &cookie)
            .await
            .map_err(|_| fail(500, "保存 IMA 凭证失败"))?;
        db.set_setting("ima_cookie_updated_at", &now_secs().to_string())
            .await
            .map_err(|_| fail(500, "保存 IMA 凭证失败"))?;
    }
    if !client_id.is_empty() {
        db.set_setting("ima_openapi_clientid", &client_id)
            .await
            .map_err(|_| fail(500, "保存 IMA 凭证失败"))?;
        db.set_setting("ima_openapi_apikey", &api_key)
            .await
            .map_err(|_| fail(500, "保存 IMA 凭证失败"))?;
    }
    Ok(())
}

pub async fn libraries(db: &Db) -> Result<Value, AdminError> {
    let mut payload = stored(db).await?;
    attach_acl(db, &mut payload).await?;
    Ok(payload)
}

pub async fn scan(db: &Db, archive: &Path) -> Result<Value, AdminError> {
    let local = archive.join("local");
    let previous = stored(db).await?;
    let found = match fs::read_dir(&local) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let payload = json!({"scanned_at": iso_now(), "status": "finished", "libraries": []});
            save_libraries(db, &payload).await?;
            return Ok(payload);
        }
        Err(_) => {
            let mut payload = previous;
            payload["status"] = json!("scan_failed");
            return Ok(payload);
        }
    };
    let mut libraries = Vec::new();
    for entry in found.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let slug = entry.file_name().to_string_lossy().to_string();
        if slug.starts_with('.') {
            continue;
        }
        if let Some(item) = read_library(&path, &slug) {
            libraries.push(item);
        }
    }
    libraries.sort_by(|left, right| {
        left["slug"]
            .as_str()
            .unwrap_or("")
            .cmp(right["slug"].as_str().unwrap_or(""))
    });
    let payload = json!({"scanned_at": iso_now(), "status": "finished", "libraries": libraries});
    save_libraries(db, &payload).await?;
    let mut payload = payload;
    attach_acl(db, &mut payload).await?;
    Ok(payload)
}

pub async fn create(
    db: &Db,
    archive: &Path,
    slug: &str,
    name: &str,
    tags: &[String],
) -> Result<Value, AdminError> {
    let slug = slug.trim().to_ascii_lowercase();
    if !slug_ok(&slug) {
        return Err(bad("slug 需为小写字母/数字/短横线（1-47 位）"));
    }
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(bad("name 需为 1-80 字"));
    }
    let tags = clean_tags(tags)?;
    let dir = archive.join("local").join(&slug);
    fs::create_dir_all(archive.join("local")).map_err(|_| fail(502, "存储归档写入失败"))?;
    match fs::create_dir(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(AdminError {
                status: 409,
                detail: format!("本地库已存在：{slug}"),
            });
        }
        Err(_) => return Err(fail(502, "存储归档写入失败")),
    }
    write_marker(&dir, name, false, &tags)?;
    scan(db, archive).await
}

pub async fn set_enabled(
    db: &Db,
    archive: &Path,
    slug: &str,
    enabled: bool,
) -> Result<Value, AdminError> {
    let dir = library_dir(archive, slug)?;
    let mut marker = read_marker(&dir)?;
    marker["enabled"] = json!(enabled);
    write_marker_value(&dir, &marker)?;
    scan(db, archive).await
}

pub async fn update(
    db: &Db,
    archive: &Path,
    slug: &str,
    name: Option<&str>,
    tags: Option<&[String]>,
) -> Result<Value, AdminError> {
    if name.is_none() && tags.is_none() {
        return Err(bad("name 与 tags 至少填一项"));
    }
    let dir = library_dir(archive, slug)?;
    let mut marker = read_marker(&dir)?;
    if let Some(name) = name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(bad("name 需为 1-80 字"));
        }
        marker["name"] = json!(name);
    }
    if let Some(tags) = tags {
        marker["tags"] = json!(clean_tags(tags)?);
    }
    write_marker_value(&dir, &marker)?;
    scan(db, archive).await
}

pub fn archive_root() -> Result<PathBuf, AdminError> {
    let root = std::env::var("IMA_ARCHIVE_ROOT").unwrap_or_default();
    let root = root.trim();
    if root.is_empty() {
        return Err(fail(503, "当前部署未挂载存储归档"));
    }
    Ok(PathBuf::from(root))
}

fn read_library(path: &Path, slug: &str) -> Option<Value> {
    let mut item = json!({
        "slug": slug,
        "group_id": format!("local-{slug}"),
        "name": slug,
        "enabled": false,
        "pdf_count": 0,
        "tags": [],
        "error": "",
    });
    if !slug_ok(slug) {
        item["error"] = json!("目录名不符合本地库 slug 规则（小写字母/数字/短横线，1-47 位）");
        return Some(item);
    }
    let marker = match fs::read_to_string(path.join(MARKER)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            item["error"] = json!("标记文件不可读");
            return Some(item);
        }
    };
    let Ok(marker) = serde_json::from_str::<Value>(&marker) else {
        item["error"] = json!("标记文件不是有效 JSON");
        return Some(item);
    };
    let name = marker["name"].as_str().unwrap_or("").trim();
    if name.is_empty() || name.chars().count() > 80 {
        item["error"] = json!("标记文件 name 需为 1-80 字");
        return Some(item);
    }
    item["name"] = json!(name);
    item["enabled"] = json!(marker["enabled"] == true);
    item["tags"] = marker["tags"]
        .as_array()
        .map(|tags| {
            Value::Array(
                tags.iter()
                    .filter_map(|tag| tag.as_str().map(str::trim))
                    .filter(|tag| !tag.is_empty())
                    .map(|tag| json!(tag))
                    .collect(),
            )
        })
        .unwrap_or(json!([]));
    item["pdf_count"] = json!(count_pdfs(path));
    Some(item)
}

fn count_pdfs(root: &Path) -> i64 {
    let mut count = 0_i64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    pending.push(entry.path());
                }
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.ends_with(".pdf") {
                count += 1;
            }
        }
    }
    count
}

fn library_dir(archive: &Path, slug: &str) -> Result<PathBuf, AdminError> {
    if !slug_ok(slug) {
        return Err(AdminError {
            status: 404,
            detail: "本地库不存在".into(),
        });
    }
    let dir = archive.join("local").join(slug);
    if !dir.join(MARKER).is_file() {
        return Err(AdminError {
            status: 404,
            detail: "本地库不存在".into(),
        });
    }
    Ok(dir)
}

fn read_marker(dir: &Path) -> Result<Value, AdminError> {
    let text = fs::read_to_string(dir.join(MARKER)).map_err(|_| fail(404, "本地库不存在"))?;
    serde_json::from_str(&text).map_err(|_| fail(400, "标记文件不是有效 JSON"))
}

fn write_marker(dir: &Path, name: &str, enabled: bool, tags: &[String]) -> Result<(), AdminError> {
    write_marker_value(
        dir,
        &json!({"name": name, "enabled": enabled, "tags": tags}),
    )
}

fn write_marker_value(dir: &Path, marker: &Value) -> Result<(), AdminError> {
    let tmp = dir.join(format!(".{}.tmp", std::process::id()));
    fs::write(&tmp, marker.to_string()).map_err(|_| fail(502, "标记文件写入失败"))?;
    fs::rename(&tmp, dir.join(MARKER)).map_err(|_| fail(502, "标记文件写入失败"))
}

async fn stored(db: &Db) -> Result<Value, AdminError> {
    let raw = db
        .setting(SETTING)
        .await
        .map_err(|_| fail(500, "读取本地库失败"))?
        .unwrap_or_default();
    let mut payload = serde_json::from_str::<Value>(&raw)
        .unwrap_or_else(|_| json!({"scanned_at": "", "libraries": []}));
    if !payload["libraries"].is_array() {
        payload["libraries"] = json!([]);
    }
    if payload["scanned_at"].as_str().is_none() {
        payload["scanned_at"] = json!("");
    }
    Ok(payload)
}

async fn save_libraries(db: &Db, payload: &Value) -> Result<(), AdminError> {
    let mut stored = payload.clone();
    stored.as_object_mut().map(|obj| obj.remove("status"));
    db.set_setting(SETTING, &stored.to_string())
        .await
        .map_err(|_| fail(500, "保存本地库失败"))
}

async fn attach_acl(db: &Db, payload: &mut Value) -> Result<(), AdminError> {
    let Some(rows) = payload["libraries"].as_array_mut() else {
        return Ok(());
    };
    for row in rows {
        let group = row["group_id"].as_str().unwrap_or("").to_string();
        let names = db
            .ima_kb_acl_usernames(&group)
            .await
            .map_err(|_| fail(500, "读取本地库失败"))?;
        row["acl_usernames"] = json!(names);
    }
    Ok(())
}

fn slug_ok(slug: &str) -> bool {
    let mut chars = slug.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && slug.len() <= 47
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn clean_tags(tags: &[String]) -> Result<Vec<String>, AdminError> {
    let mut out = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() || out.iter().any(|item: &String| item == tag) {
            continue;
        }
        if tag.chars().count() > 40 || tag.chars().any(|c| c.is_control()) {
            return Err(bad("标签无效"));
        }
        if out.len() >= 20 {
            return Err(bad("标签过多"));
        }
        out.push(tag.to_string());
    }
    Ok(out)
}

fn clean(raw: &str, limit: usize, message: &'static str) -> Result<String, AdminError> {
    let value = raw.trim();
    if value.len() > limit || value.chars().any(|c| c.is_control()) {
        return Err(bad(message));
    }
    Ok(value.to_string())
}

fn preview(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() > 12 {
        format!("{}…", chars.iter().take(12).collect::<String>())
    } else {
        value.to_string()
    }
}

async fn saved(db: &Db, key: &str) -> Result<String, sqlx::Error> {
    Ok(db.setting(key).await?.unwrap_or_default())
}

fn env_text(key: &str) -> String {
    std::env::var(key).unwrap_or_default().trim().to_string()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn iso_now() -> String {
    chrono_lite(now_secs())
}

fn chrono_lite(secs: u64) -> String {
    // 1970-01-01 UTC plus seconds. The page only checks that this parses as a date.
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (year, month, day) = civil_date(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
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

fn bad(detail: &'static str) -> AdminError {
    fail(400, detail)
}

fn fail(status: u16, detail: &'static str) -> AdminError {
    AdminError {
        status,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (Db, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "vpush-ima-admin-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (Db::open(&path).await.unwrap(), path)
    }

    #[tokio::test]
    async fn credentials_are_saved_but_not_returned() {
        let (db, path) = db().await;
        let err = save_credentials(&db, "", "only-id", "").await.unwrap_err();
        assert_eq!(
            err.detail,
            "需至少提供 ima Cookie 或 OpenAPI 凭证（clientid + apikey）"
        );
        let err = save_credentials(&db, "IMA-TOKEN=secret", "only-id", "")
            .await
            .unwrap_err();
        assert_eq!(err.detail, "OpenAPI 凭证需同时提供 clientid 与 apikey");
        save_credentials(
            &db,
            "IMA-TOKEN=secret",
            "client-id-preview",
            "api-key-secret",
        )
        .await
        .unwrap();
        let view = credentials(&db).await.unwrap();
        let body = view.to_string();
        assert!(!body.contains("secret"));
        assert!(!body.contains("api-key"));
        assert_eq!(view["mode"], "openapi");
        assert_eq!(view["cookie"]["preview"], "已配置");
        assert_eq!(view["cookie"]["from_env"], false);
        assert_eq!(view["openapi_clientid"]["preview"], "client-id-pr…");
        assert_eq!(view["openapi_apikey"]["set"], true);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn local_library_round_trip_counts_pdfs() {
        let (db, db_path) = db().await;
        let root = std::env::temp_dir().join(format!(
            "vpush-ima-libs-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let created = create(
            &db,
            &root,
            "My-Papers",
            "论文库",
            &["研报".into(), "研报".into()],
        )
        .await
        .unwrap();
        let lib = &created["libraries"][0];
        assert_eq!(lib["slug"], "my-papers");
        assert_eq!(lib["name"], "论文库");
        assert_eq!(lib["enabled"], false);
        assert_eq!(lib["pdf_count"], 0);
        assert_eq!(lib["tags"], json!(["研报"]));
        fs::write(root.join("local/my-papers/note.pdf"), b"%PDF").unwrap();
        fs::write(root.join("local/my-papers/skip.txt"), b"no").unwrap();
        let updated = update(&db, &root, "my-papers", Some("新论文库"), None)
            .await
            .unwrap();
        assert_eq!(updated["libraries"][0]["name"], "新论文库");
        assert_eq!(updated["libraries"][0]["pdf_count"], 1);
        let enabled = set_enabled(&db, &root, "my-papers", true).await.unwrap();
        assert_eq!(enabled["libraries"][0]["enabled"], true);
        assert!(create(&db, &root, "my-papers", "重复", &[]).await.is_err());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(db_path);
    }
}
