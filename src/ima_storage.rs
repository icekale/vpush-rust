use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::db::Db;

pub const REPORT_KEY: &str = "ima_storage_consistency_report";
pub const REFRESH_KEY: &str = "ima_storage_refresh_requested_at";
pub const ALERTS_KEY: &str = "ima_storage_alert_settings";

#[derive(Debug)]
pub struct StorageError {
    pub status: u16,
    pub detail: String,
}

impl StorageError {
    fn bad(detail: impl Into<String>) -> Self {
        Self {
            status: 400,
            detail: detail.into(),
        }
    }

    fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            status: 503,
            detail: detail.into(),
        }
    }

    fn internal(detail: impl Into<String>) -> Self {
        Self {
            status: 500,
            detail: detail.into(),
        }
    }
}

pub fn archive_root() -> Result<PathBuf, StorageError> {
    let raw = std::env::var("IMA_ARCHIVE_ROOT").unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(StorageError::unavailable("当前部署未挂载存储归档"));
    }
    let root = PathBuf::from(raw);
    if !root.is_dir() {
        return Err(StorageError::unavailable("知识库存储归档不可用"));
    }
    Ok(root)
}

pub fn safe_child(root: &Path, relative: &str) -> Result<PathBuf, StorageError> {
    let relative = Path::new(relative);
    if relative.is_absolute() || relative.as_os_str().is_empty() {
        return Err(StorageError::bad("归档路径无效"));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                current.push(part);
                if current
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    return Err(StorageError::bad("归档路径不能包含符号链接"));
                }
            }
            Component::CurDir => {}
            _ => return Err(StorageError::bad("归档路径不能越界")),
        }
    }
    Ok(current)
}

pub async fn health(db: &Db, root: &Path) -> Result<Value, StorageError> {
    let (files, bytes) = scan_files(root)?;
    let documents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ima_document_index")
        .fetch_one(db.pool())
        .await
        .map_err(|_| StorageError::internal("读取存储统计失败"))?;
    let writable = !root
        .metadata()
        .map_err(|_| StorageError::unavailable("知识库存储归档不可读"))?
        .permissions()
        .readonly();
    Ok(json!({
        "available": true,
        "readable": true,
        "writable": writable,
        "files": files,
        "bytes": bytes,
        "capacity_bytes": available_capacity(root),
        "documents": documents,
        "last_consistency": db.setting(REPORT_KEY).await.map_err(|_| StorageError::internal("读取一致性状态失败"))?,
        "last_refresh_requested_at": db.setting(REFRESH_KEY).await.map_err(|_| StorageError::internal("读取刷新状态失败"))?,
    }))
}

pub async fn consistency(db: &Db, root: &Path) -> Result<Value, StorageError> {
    if let Some(raw) = db
        .setting(REPORT_KEY)
        .await
        .map_err(|_| StorageError::internal("读取一致性状态失败"))?
    {
        if let Ok(value) = serde_json::from_str(&raw) {
            return Ok(value);
        }
    }
    run_consistency(db, root).await
}

pub async fn run_consistency(db: &Db, root: &Path) -> Result<Value, StorageError> {
    let references = referenced_files(db).await?;
    let (files, _) = scan_files(root)?;
    let mut missing = Vec::new();
    for path in &references {
        if path.ends_with('/') {
            continue;
        }
        let absolute = safe_child(root, path)?;
        if !absolute.is_file() {
            missing.push(path.clone());
        }
    }
    missing.sort();
    let file_set: BTreeSet<_> = files.iter().map(|(path, _)| path.clone()).collect();
    let mut orphan: Vec<_> = file_set
        .iter()
        .filter(|path| {
            !references.contains(*path)
                && !references
                    .iter()
                    .filter(|reference| reference.ends_with('/'))
                    .any(|reference| path.starts_with(reference.as_str()))
        })
        .cloned()
        .collect();
    orphan.sort();
    let report = json!({
        "checked_at": now(),
        "referenced": references.len(),
        "files": files.len(),
        "missing": missing,
        "orphan": orphan,
        "ok": missing.is_empty(),
    });
    db.set_setting(REPORT_KEY, &report.to_string())
        .await
        .map_err(|_| StorageError::internal("保存一致性结果失败"))?;
    Ok(report)
}

pub async fn dedup(db: &Db, root: &Path) -> Result<Value, StorageError> {
    let references = referenced_files(db).await?;
    let (files, _) = scan_files(root)?;
    let mut by_hash: BTreeMap<Vec<u8>, Vec<String>> = BTreeMap::new();
    for (path, _) in files {
        if references.contains(&path)
            || references
                .iter()
                .filter(|reference| reference.ends_with('/'))
                .any(|reference| path.starts_with(reference.as_str()))
        {
            continue;
        }
        let absolute = safe_child(root, &path)?;
        let digest = Sha256::digest(
            fs::read(&absolute).map_err(|_| StorageError::internal("读取归档文件失败"))?,
        );
        by_hash.entry(digest.to_vec()).or_default().push(path);
    }
    let mut removed = Vec::new();
    for paths in by_hash.values_mut() {
        paths.sort();
        for path in paths.iter().skip(1) {
            let absolute = safe_child(root, path)?;
            if absolute
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            fs::remove_file(absolute)
                .map_err(|_| StorageError::internal("删除重复归档文件失败"))?;
            removed.push(path.clone());
        }
    }
    let report = run_consistency(db, root).await?;
    Ok(json!({"removed": removed, "report": report}))
}

pub async fn refresh(db: &Db, root: &Path) -> Result<Value, StorageError> {
    let marker = safe_child(root, ".vpush-ima-refresh-request")?;
    fs::write(&marker, now()).map_err(|_| StorageError::unavailable("无法写入存储刷新请求"))?;
    db.set_setting(REFRESH_KEY, &now())
        .await
        .map_err(|_| StorageError::internal("保存刷新状态失败"))?;
    Ok(json!({"status": "requested", "requested_at": now()}))
}

pub async fn backup(_db: &Db, _root: &Path) -> Result<Value, StorageError> {
    Err(StorageError::unavailable(
        "当前部署未配置 Rust 存储备份目标",
    ))
}

pub async fn alert_settings(db: &Db) -> Result<Value, StorageError> {
    let raw = db
        .setting(ALERTS_KEY)
        .await
        .map_err(|_| StorageError::internal("读取存储告警配置失败"))?;
    Ok(raw
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(default_alerts))
}

pub async fn save_alert_settings(db: &Db, body: &Value) -> Result<Value, StorageError> {
    let settings = validate_alerts(body)?;
    db.set_setting(ALERTS_KEY, &settings.to_string())
        .await
        .map_err(|_| StorageError::internal("保存存储告警配置失败"))?;
    Ok(settings)
}

fn default_alerts() -> Value {
    json!({"enabled": true, "min_free_bytes": 1073741824_u64, "max_missing": 0, "max_orphan": 100})
}

fn validate_alerts(body: &Value) -> Result<Value, StorageError> {
    let source = body
        .as_object()
        .ok_or_else(|| StorageError::bad("告警配置必须是对象"))?;
    let defaults = default_alerts();
    let mut result = defaults;
    for key in ["enabled", "min_free_bytes", "max_missing", "max_orphan"] {
        if let Some(value) = source.get(key) {
            match key {
                "enabled" if value.is_boolean() => result[key] = value.clone(),
                "min_free_bytes"
                    if value.as_u64().is_some() && value.as_u64().unwrap_or(0) <= (1_u64 << 50) =>
                {
                    result[key] = value.clone()
                }
                "max_missing" | "max_orphan"
                    if value.as_u64().is_some() && value.as_u64().unwrap_or(0) <= 1_000_000 =>
                {
                    result[key] = value.clone()
                }
                _ => return Err(StorageError::bad("存储告警配置无效")),
            }
        }
    }
    if source.keys().any(|key| {
        !["enabled", "min_free_bytes", "max_missing", "max_orphan"].contains(&key.as_str())
    }) {
        return Err(StorageError::bad("存储告警配置包含未知字段"));
    }
    Ok(result)
}

async fn referenced_files(db: &Db) -> Result<BTreeSet<String>, StorageError> {
    let mut references = BTreeSet::new();
    for row in sqlx::query("SELECT pdf_path, txt_path FROM ima_document_index")
        .fetch_all(db.pool())
        .await
        .map_err(|_| StorageError::internal("读取归档引用失败"))?
    {
        for key in ["pdf_path", "txt_path"] {
            if let Ok(path) = row.try_get::<String, _>(key) {
                if !path.trim().is_empty() {
                    references.insert(path);
                }
            }
        }
    }
    for row in sqlx::query("SELECT timeline_path, asset_root FROM feishu_document_sources")
        .fetch_all(db.pool())
        .await
        .map_err(|_| StorageError::internal("读取文档引用失败"))?
    {
        for (key, is_root) in [("timeline_path", false), ("asset_root", true)] {
            if let Ok(path) = row.try_get::<String, _>(key) {
                if !path.trim().is_empty() {
                    let path = path.trim().trim_end_matches('/');
                    references.insert(if is_root {
                        format!("{path}/")
                    } else {
                        path.to_string()
                    });
                }
            }
        }
    }
    Ok(references)
}

fn scan_files(root: &Path) -> Result<(Vec<(String, u64)>, u64), StorageError> {
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    let bytes = files.iter().map(|(_, size)| *size).sum();
    Ok((files, bytes))
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, u64)>) -> Result<(), StorageError> {
    for entry in fs::read_dir(dir).map_err(|_| StorageError::unavailable("读取知识库存储失败"))?
    {
        let entry = entry.map_err(|_| StorageError::internal("读取知识库存储失败"))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|_| StorageError::internal("读取归档元数据失败"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(root, &path, files)?;
        } else if meta.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| StorageError::internal("归档路径计算失败"))?
                .to_string_lossy()
                .replace('\\', "/");
            if !relative.starts_with(".vpush-") {
                files.push((relative, meta.len()));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn available_capacity(root: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(root.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    stats.f_bavail.checked_mul(stats.f_frsize)
}

#[cfg(not(unix))]
fn available_capacity(_root: &Path) -> Option<u64> {
    None
}

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn consistency_reports_missing_orphan_and_dedupes_unreferenced_files() {
        let root = tempfile_dir("consistency");
        fs::create_dir_all(root.join("local")).unwrap();
        fs::write(root.join("local/a.pdf"), b"referenced").unwrap();
        fs::write(root.join("local/orphan.pdf"), b"orphan").unwrap();
        fs::write(root.join("local/duplicate-a.pdf"), b"duplicate").unwrap();
        fs::write(root.join("local/duplicate-b.pdf"), b"duplicate").unwrap();

        let db_path =
            std::env::temp_dir().join(format!("vpush-ima-storage-db-{}", std::process::id()));
        let db = crate::db::Db::open(&db_path).await.unwrap();
        sqlx::query("INSERT INTO ima_document_index (group_id, media_id, pdf_path, txt_path) VALUES ('g', 'm', 'local/a.pdf', 'local/missing.txt')")
            .execute(db.pool())
            .await
            .unwrap();

        let report = run_consistency(&db, &root).await.unwrap();
        assert_eq!(report["missing"], json!(["local/missing.txt"]));
        assert_eq!(
            report["orphan"],
            json!([
                "local/duplicate-a.pdf",
                "local/duplicate-b.pdf",
                "local/orphan.pdf"
            ])
        );

        let dedup = dedup(&db, &root).await.unwrap();
        assert_eq!(dedup["removed"], json!(["local/duplicate-b.pdf"]));
        assert!(root.join("local/duplicate-a.pdf").is_file());
        assert!(root.join("local/a.pdf").is_file());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn alert_thresholds_are_bounded_and_reject_unknown_fields() {
        assert!(validate_alerts(&json!({"min_free_bytes": -1})).is_err());
        assert!(validate_alerts(&json!({"unknown": true})).is_err());
        assert!(validate_alerts(&json!({"max_orphan": 100})).is_ok());
    }

    #[test]
    fn archive_paths_reject_symlinks_and_escape() {
        let root = tempfile_dir("path");
        fs::create_dir_all(root.join("local")).unwrap();
        fs::write(root.join("outside"), b"secret").unwrap();
        symlink(root.join("outside"), root.join("local/link")).unwrap();
        assert!(safe_child(&root, "../outside").is_err());
        assert!(safe_child(&root, "local/link/file").is_err());
        let _ = fs::remove_dir_all(root);
    }

    fn tempfile_dir(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("vpush-ima-storage-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
