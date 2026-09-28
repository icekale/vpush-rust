//! 飞书文档管理页：应用配置、授权、来源记录、正文时间线和媒体文件。
//! 单个文件超过 50 MB，或一份文档的媒体总量超过 250 MB，同步失败。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use base64::engine::general_purpose::URL_SAFE;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::db::Db;
use crate::feishu_personal::{credential_key, open_app_secret, seal};

const AUTHORIZE: &str = "https://accounts.feishu.cn/open-apis/authen/v1/authorize";
const TOKEN_URL: &str = "https://open.feishu.cn/open-apis/authen/v2/oauth/token";
const DEFAULT_SCOPES: &str =
    "wiki:node:read docx:document:readonly docs:document.media:download offline_access";

#[derive(Debug)]
pub struct Fail {
    pub status: u16,
    pub detail: &'static str,
}

pub fn parse_url(raw: &str) -> Result<Parsed, Fail> {
    let raw = raw.trim();
    let rest = raw
        .strip_prefix("https://")
        .ok_or(fail(400, "只支持飞书或 Lark 的 HTTPS 文档链接"))?;
    let (host, path) = rest
        .split_once('/')
        .ok_or(fail(400, "链接必须是 /wiki/{token} 或 /docx/{token}"))?;
    let host = host
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !trusted_host(&host) {
        return Err(fail(400, "只支持飞书或 Lark 的 HTTPS 文档链接"));
    }
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let kind = parts.next().unwrap_or("");
    let token = parts
        .next()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if parts.next().is_some() || (kind != "wiki" && kind != "docx") || !valid_token(token) {
        return Err(fail(400, "链接必须是 /wiki/{token} 或 /docx/{token}"));
    }
    let source_key = format!("{host}:{kind}:{token}");
    let digest = hex(&Sha256::digest(source_key.as_bytes()))[..20].to_string();
    Ok(Parsed {
        host: host.clone(),
        source_type: kind.to_string(),
        source_token: token.to_string(),
        canonical_url: format!("https://{host}/{kind}/{token}"),
        source_key_hash: digest.clone(),
        group_id: format!("feishu-{digest}"),
        media_id: format!("fs{digest}"),
    })
}

pub struct Parsed {
    host: String,
    pub source_type: String,
    pub source_token: String,
    canonical_url: String,
    source_key_hash: String,
    group_id: String,
    media_id: String,
}

pub async fn overview(db: &Db) -> Result<Value, Fail> {
    let cfg = config(db).await?;
    Ok(json!({
        "configured": cfg.configured,
        "authorized": credential_live(db).await?,
        "interval_seconds": cfg.interval,
        "config": cfg.public,
        "sources": list_sources(db, cfg.interval).await?,
    }))
}

pub struct Save<'a> {
    pub app_id: Option<&'a str>,
    pub app_secret: Option<&'a str>,
    pub redirect_uri: Option<&'a str>,
    pub scopes: Option<&'a str>,
    pub interval_seconds: Option<i64>,
}

pub async fn save_config(db: &Db, input: Save<'_>) -> Result<Value, Fail> {
    let mut changed_credentials = false;
    let mut wrote = false;
    if let Some(app_id) = input.app_id {
        let app_id = app_id.trim();
        if app_id.len() > 128 {
            return Err(fail(400, "App ID 过长"));
        }
        let previous = text(db, "feishu_docs_app_id").await?;
        if app_id != previous {
            changed_credentials = true;
        }
        db.set_setting("feishu_docs_app_id", app_id)
            .await
            .map_err(|_| fail(400, "飞书文档配置保存失败"))?;
        wrote = true;
    }
    if let Some(secret) = input
        .app_secret
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if secret.len() > 256 {
            return Err(fail(400, "App Secret 过长"));
        }
        let key = credential_key().ok_or(fail(400, "未配置 FEISHU_CREDENTIAL_KEY"))?;
        let sealed = seal(&key, secret).map_err(|_| fail(400, "无法加密 App Secret"))?;
        db.set_setting("feishu_docs_app_secret", &sealed)
            .await
            .map_err(|_| fail(400, "飞书文档配置保存失败"))?;
        changed_credentials = true;
        wrote = true;
    }
    if let Some(redirect) = input.redirect_uri {
        let redirect = redirect.trim();
        if !redirect.is_empty() && !redirect.starts_with("https://") {
            return Err(fail(400, "回调地址必须是 HTTPS"));
        }
        if redirect.len() > 512 {
            return Err(fail(400, "回调地址过长"));
        }
        db.set_setting("feishu_docs_redirect_uri", redirect)
            .await
            .map_err(|_| fail(400, "飞书文档配置保存失败"))?;
        wrote = true;
    }
    if let Some(scopes) = input.scopes {
        let scopes = scopes.split_whitespace().collect::<Vec<_>>().join(" ");
        if scopes.is_empty() {
            return Err(fail(400, "授权权限不能为空"));
        }
        if scopes.len() > 500 {
            return Err(fail(400, "授权权限列表过长"));
        }
        db.set_setting("feishu_docs_scopes", &scopes)
            .await
            .map_err(|_| fail(400, "飞书文档配置保存失败"))?;
        wrote = true;
    }
    if let Some(interval) = input.interval_seconds {
        if !(15..=86400).contains(&interval) {
            return Err(fail(400, "检查间隔需在 15–86400 秒之间"));
        }
        db.set_setting("feishu_docs_interval_seconds", &interval.to_string())
            .await
            .map_err(|_| fail(400, "飞书文档配置保存失败"))?;
        wrote = true;
    }
    if !wrote {
        return Err(fail(400, "没有要保存的配置"));
    }
    let had_credential = credential_live(db).await?;
    if changed_credentials && had_credential {
        sqlx::query("DELETE FROM feishu_oauth_credentials")
            .execute(db.pool())
            .await
            .map_err(|_| fail(400, "飞书文档配置保存失败"))?;
    }
    let cfg = config(db).await?;
    Ok(json!({
        "ok": true,
        "config": cfg.public,
        "reauth_required": changed_credentials && had_credential,
    }))
}

pub async fn begin_oauth(db: &Db, user_id: i64) -> Result<(String, String), Fail> {
    let cfg = config(db).await?;
    if !cfg.configured {
        return Err(fail(400, "飞书文档应用尚未配置"));
    }
    let state = random_token(32)?;
    let verifier = random_token(48)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state_hash = hex(&Sha256::digest(state.as_bytes()));
    let expires = now() + 300;
    sqlx::query("INSERT INTO feishu_oauth_sessions (state_hash, user_id, code_verifier, expires_at) VALUES (?, ?, ?, ?)")
        .bind(&state_hash)
        .bind(user_id)
        .bind(&verifier)
        .bind(expires as i64)
        .execute(db.pool())
        .await
        .map_err(|_| fail(400, "无法开始飞书授权"))?;
    let url = format!(
        "{AUTHORIZE}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        encode(&cfg.app_id),
        encode(&cfg.redirect),
        encode(&cfg.scopes),
        encode(&state),
        encode(&challenge)
    );
    Ok((url, state_hash))
}

pub async fn take_session(db: &Db, state: &str, cookie_hash: &str) -> Result<String, Fail> {
    let state = state.trim();
    let state_hash = hex(&Sha256::digest(state.as_bytes()));
    if state.is_empty() || cookie_hash != state_hash {
        return Err(fail(400, "飞书授权请求已过期或已使用"));
    }
    let row = sqlx::query(
        "SELECT user_id, code_verifier, expires_at FROM feishu_oauth_sessions WHERE state_hash = ?",
    )
    .bind(&state_hash)
    .fetch_optional(db.pool())
    .await
    .map_err(|_| fail(400, "飞书授权失败"))?;
    let Some(row) = row else {
        return Err(fail(400, "飞书授权请求已过期或已使用"));
    };
    let expires: i64 = row.get("expires_at");
    let user_id: i64 = row.get("user_id");
    let verifier: String = row.get("code_verifier");
    sqlx::query("DELETE FROM feishu_oauth_sessions WHERE state_hash = ?")
        .bind(&state_hash)
        .execute(db.pool())
        .await
        .map_err(|_| fail(400, "飞书授权失败"))?;
    if expires <= now() as i64 {
        return Err(fail(400, "飞书授权请求已过期或已使用"));
    }
    let admin = db
        .user_by_id(user_id)
        .await
        .map_err(|_| fail(400, "飞书授权失败"))?;
    if !admin.is_some_and(|user| user.is_admin) {
        return Err(fail(403, "授权发起账号不是管理员"));
    }
    Ok(verifier)
}

pub async fn save_token(db: &Db, token_body: &str) -> Result<(), Fail> {
    let token = parse_token(token_body)?;
    let key = credential_key().ok_or(fail(400, "未配置 FEISHU_CREDENTIAL_KEY"))?;
    let access = seal(&key, &token.access).map_err(|_| fail(400, "无法保存飞书授权"))?;
    let refresh = seal(&key, &token.refresh).map_err(|_| fail(400, "无法保存飞书授权"))?;
    sqlx::query(
        "INSERT INTO feishu_oauth_credentials (id, access_token, refresh_token, expires_at, refresh_expires_at) VALUES (1, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET access_token = excluded.access_token, refresh_token = excluded.refresh_token, expires_at = excluded.expires_at, refresh_expires_at = excluded.refresh_expires_at",
    )
    .bind(access)
    .bind(refresh)
    .bind(now() as i64 + token.expires_in)
    .bind(now() as i64 + token.refresh_expires_in)
    .execute(db.pool())
    .await
    .map_err(|_| fail(400, "无法保存飞书授权"))?;
    Ok(())
}

pub fn exchange_refresh(app_id: &str, secret: &str, refresh: &str) -> Result<String, Fail> {
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "client_id": app_id,
        "client_secret": secret,
    });
    post_token(&body)
}

pub fn exchange_code(
    app_id: &str,
    secret: &str,
    redirect: &str,
    code: &str,
    verifier: &str,
) -> Result<String, Fail> {
    let body = json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect,
        "code_verifier": verifier,
        "client_id": app_id,
        "client_secret": secret,
    });
    post_token(&body)
}

fn post_token(body: &Value) -> Result<String, Fail> {
    let response = ureq::post(TOKEN_URL)
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(20))
        .send_string(&body.to_string())
        .map_err(|_| fail(400, "飞书授权失败"))?;
    response
        .into_string()
        .map_err(|_| fail(400, "飞书授权失败"))
}

pub async fn preview_url(db: &Db, url: &str) -> Result<Value, Fail> {
    let parsed = parse_url(url)?;
    let meta = read_meta(db, &parsed.source_token, &parsed.source_type).await?;
    preview(db, url, Some(meta)).await
}

pub async fn sync_source(db: &Db, id: i64) -> Result<Value, Fail> {
    let row = sqlx::query(
        "SELECT source_token, source_type, source_key_hash, group_id, media_id, display_name, revision_id, timeline_path, enabled
         FROM feishu_document_sources WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await
    .map_err(|_| fail(400, "读取飞书文档失败"))?
    .ok_or(fail(404, "飞书文档来源不存在"))?;
    if row.get::<i64, _>("enabled") == 0 {
        return Err(fail(400, "请先启用该来源"));
    }
    if !credential_live(db).await? {
        return Err(fail(400, "请先授权飞书文档"));
    }
    let source_token = row.get::<String, _>("source_token");
    let kind = row.get::<String, _>("source_type");
    let meta = match read_meta(db, &source_token, &kind).await {
        Ok(meta) => meta,
        Err(err) => {
            mark_failed(db, id, err.detail).await?;
            return Err(err);
        }
    };
    let revision = meta["revision_id"].as_str().unwrap_or("-1");
    let stored_revision = row.get::<String, _>("revision_id");
    let stored_timeline = row.get::<String, _>("timeline_path");
    if revision == stored_revision && !stored_timeline.is_empty() {
        touch_sync(db, id, "succeeded", "").await?;
        return Ok(json!({"ok": true, "status": "unchanged"}));
    }
    let document_id = meta["document_id"].as_str().unwrap_or("").to_string();
    let access = access_token(db).await?;
    let blocks = {
        let access = access.clone();
        tokio::task::spawn_blocking(move || fetch_blocks(&document_id, &access))
            .await
            .map_err(|_| fail(500, "飞书文档读取失败"))?
    };
    let blocks = match blocks {
        Ok(blocks) => blocks,
        Err(err) => {
            mark_failed(db, id, err.detail).await?;
            return Err(err);
        }
    };
    let timeline = normalize_blocks(&blocks);
    let display = row.get::<String, _>("display_name");
    let title = if display.trim().is_empty() {
        meta["title"].as_str().unwrap_or("飞书文档")
    } else {
        display.trim()
    };
    let title = clip(title, 200);
    let root = archive_root()?;
    let key_hash: String = row.get("source_key_hash");
    let title_for_files = title.clone();
    let access_for_files = access.clone();
    let published = tokio::task::spawn_blocking(move || {
        let mut timeline = timeline;
        publish_files(
            &root,
            &key_hash,
            &title_for_files,
            &mut timeline,
            &mut |token| download_media(token, &access_for_files),
        )
        .map(|paths| (paths, timeline))
    })
    .await
    .map_err(|_| fail(500, "飞书文档读取失败"))?;
    let ((timeline_path, txt_path, asset_root), timeline) = match published {
        Ok(paths) => paths,
        Err(err) => {
            mark_failed(db, id, err.detail).await?;
            return Err(err);
        }
    };
    let count = timeline["entries"]
        .as_array()
        .map(|items| items.len() as i64)
        .unwrap_or(0);
    let day = timeline["entries"]
        .as_array()
        .and_then(|items| items.last())
        .and_then(|item| item["day"].as_str())
        .unwrap_or("");
    let now = chrono_like_now();
    sqlx::query(
        "UPDATE feishu_document_sources
         SET title = ?, revision_id = ?, timeline_path = ?, asset_root = ?, entry_count = ?, sync_status = 'succeeded', last_checked_at = ?, last_success_at = ?, last_error = ''
         WHERE id = ?",
    )
    .bind(&title)
    .bind(revision)
    .bind(&timeline_path)
    .bind(&asset_root)
    .bind(count)
    .bind(&now)
    .bind(&now)
    .bind(id)
    .execute(db.pool())
    .await
    .map_err(|_| fail(400, "飞书文档读取失败"))?;
    let plain = plain_text(&title, &timeline);
    sqlx::query(
        "INSERT INTO ima_document_index (group_id, media_id, day, sort_date, name, group_name, size, chars, txt_path, downloaded_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(group_id, media_id) DO UPDATE SET day = excluded.day, sort_date = excluded.sort_date, name = excluded.name,
         group_name = excluded.group_name, size = excluded.size, chars = excluded.chars, txt_path = excluded.txt_path, downloaded_at = excluded.downloaded_at",
    )
    .bind(row.get::<String, _>("group_id"))
    .bind(row.get::<String, _>("media_id"))
    .bind(day)
    .bind(day)
    .bind(&title)
    .bind(&title)
    .bind(plain.len() as i64)
    .bind(plain.chars().count() as i64)
    .bind(&txt_path)
    .bind(&now)
    .execute(db.pool())
    .await
    .map_err(|_| fail(400, "飞书文档读取失败"))?;
    Ok(json!({"ok": true, "status": "updated", "entry_count": count}))
}

async fn touch_sync(db: &Db, id: i64, status: &str, error: &str) -> Result<(), Fail> {
    sqlx::query("UPDATE feishu_document_sources SET sync_status = ?, last_error = ?, last_checked_at = ? WHERE id = ?")
        .bind(status)
        .bind(error)
        .bind(chrono_like_now())
        .bind(id)
        .execute(db.pool())
        .await
        .map_err(|_| fail(400, "飞书文档读取失败"))?;
    Ok(())
}

async fn mark_failed(db: &Db, id: i64, error: &str) -> Result<(), Fail> {
    touch_sync(db, id, "failed", error).await
}

pub async fn preview(db: &Db, url: &str, meta: Option<Value>) -> Result<Value, Fail> {
    let parsed = parse_url(url)?;
    let cfg = config(db).await?;
    if !cfg.configured {
        return Err(fail(400, "飞书文档应用尚未配置"));
    }
    if !credential_live(db).await? {
        return Err(fail(400, "请先授权飞书文档"));
    }
    let meta = meta.ok_or(fail(400, "飞书文档读取失败"))?;
    Ok(json!({
        "source_type": parsed.source_type,
        "title": clip(meta["title"].as_str().unwrap_or("飞书文档"), 200),
        "revision_id": meta["revision_id"].as_str().unwrap_or("-1"),
        "ready": true,
    }))
}

pub async fn add_source(db: &Db, url: &str) -> Result<Value, Fail> {
    let parsed = parse_url(url)?;
    let existing =
        sqlx::query("SELECT id, deleted_at FROM feishu_document_sources WHERE source_key_hash = ?")
            .bind(&parsed.source_key_hash)
            .fetch_optional(db.pool())
            .await
            .map_err(|_| fail(400, "添加飞书文档失败"))?;
    if let Some(row) = existing {
        let id: i64 = row.get("id");
        if row.get::<Option<String>, _>("deleted_at").is_some() {
            sqlx::query("UPDATE feishu_document_sources SET deleted_at = NULL, enabled = 1, sync_status = 'pending', last_error = '' WHERE id = ?")
                .bind(id)
                .execute(db.pool())
                .await
                .map_err(|_| fail(400, "添加飞书文档失败"))?;
        }
        return source_json(db, id).await;
    }
    let id = sqlx::query(
        "INSERT INTO feishu_document_sources (source_key_hash, group_id, media_id, title, canonical_url, source_type, source_token, host, enabled, sync_status)
         VALUES (?, ?, ?, '待首次同步', ?, ?, ?, ?, 1, 'pending')",
    )
    .bind(&parsed.source_key_hash)
    .bind(&parsed.group_id)
    .bind(&parsed.media_id)
    .bind(&parsed.canonical_url)
    .bind(&parsed.source_type)
    .bind(&parsed.source_token)
    .bind(&parsed.host)
    .execute(db.pool())
    .await
    .map_err(|_| fail(400, "添加飞书文档失败"))?
    .last_insert_rowid();
    source_json(db, id).await
}

pub async fn update_source(
    db: &Db,
    id: i64,
    enabled: Option<bool>,
    display_mode: Option<&str>,
    display_name: Option<&str>,
) -> Result<Value, Fail> {
    if enabled.is_none() && display_mode.is_none() && display_name.is_none() {
        return Err(fail(400, "没有要更新的字段"));
    }
    let source = source_row(db, id).await?;
    if display_mode.is_some_and(|mode| mode != "timeline" && mode != "document") {
        return Err(fail(400, "展示方式必须是 timeline 或 document"));
    }
    if let Some(name) = display_name {
        let name = name.trim();
        if name.chars().count() > 200 {
            return Err(fail(400, "展示名过长（≤200 字）"));
        }
        sqlx::query("UPDATE feishu_document_sources SET display_name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(db.pool())
            .await
            .map_err(|_| fail(400, "更新飞书文档失败"))?;
    }
    if let Some(mode) = display_mode {
        sqlx::query("UPDATE feishu_document_sources SET display_mode = ? WHERE id = ?")
            .bind(mode)
            .bind(id)
            .execute(db.pool())
            .await
            .map_err(|_| fail(400, "更新飞书文档失败"))?;
    }
    if let Some(enabled) = enabled {
        let status = if enabled { "pending" } else { "disabled" };
        sqlx::query("UPDATE feishu_document_sources SET enabled = ?, sync_status = ?, last_error = '' WHERE id = ?")
            .bind(i64::from(enabled))
            .bind(status)
            .bind(id)
            .execute(db.pool())
            .await
            .map_err(|_| fail(400, "更新飞书文档失败"))?;
    }
    let _ = source;
    source_json(db, id).await
}

pub async fn remove_source(db: &Db, id: i64) -> Result<(), Fail> {
    let changed = sqlx::query("UPDATE feishu_document_sources SET deleted_at = datetime('now'), enabled = 0, sync_status = 'disabled' WHERE id = ? AND deleted_at IS NULL")
        .bind(id)
        .execute(db.pool())
        .await
        .map_err(|_| fail(400, "移除飞书文档失败"))?;
    if changed.rows_affected() == 0 {
        return Err(fail(404, "飞书文档来源不存在"));
    }
    Ok(())
}

#[allow(dead_code)]
pub async fn note_sync(db: &Db, id: i64, meta: Option<Value>) -> Result<Value, Fail> {
    let row = source_row(db, id).await?;
    if row.get::<i64, _>("enabled") == 0 {
        return Err(fail(400, "请先启用该来源"));
    }
    if !credential_live(db).await? {
        return Err(fail(400, "请先授权飞书文档"));
    }
    let checked = chrono_like_now();
    if let Some(meta) = meta {
        let title = clip(meta["title"].as_str().unwrap_or("飞书文档"), 200);
        let revision = meta["revision_id"].as_str().unwrap_or("");
        sqlx::query("UPDATE feishu_document_sources SET title = ?, revision_id = ?, last_checked_at = ?, last_error = '', sync_status = 'pending' WHERE id = ?")
            .bind(title)
            .bind(revision)
            .bind(&checked)
            .bind(id)
            .execute(db.pool())
            .await
            .map_err(|_| fail(400, "飞书文档读取失败"))?;
    } else {
        sqlx::query("UPDATE feishu_document_sources SET last_checked_at = ?, last_error = ?, sync_status = 'failed' WHERE id = ?")
            .bind(&checked)
            .bind("飞书文档正文同步尚未完成")
            .bind(id)
            .execute(db.pool())
            .await
            .map_err(|_| fail(400, "飞书文档读取失败"))?;
    }
    Ok(json!({"ok": true, "status": "queued"}))
}

pub async fn read_meta(db: &Db, url_or_token: &str, source_type: &str) -> Result<Value, Fail> {
    let token = access_token(db).await?;
    let document_id = if source_type == "docx" {
        url_or_token.to_string()
    } else {
        let wiki = url_or_token.to_string();
        let token = token.clone();
        let body = tokio::task::spawn_blocking(move || {
            get_json(
                &format!(
                    "https://open.feishu.cn/open-apis/wiki/v2/spaces/get_node?token={}",
                    encode(&wiki)
                ),
                &token,
            )
        })
        .await
        .map_err(|_| fail(500, "飞书文档读取失败"))??;
        let node = body.get("node").cloned().unwrap_or(body);
        if node["obj_type"].as_str() != Some("docx") {
            return Err(fail(400, "当前仅支持飞书新版文档"));
        }
        node["obj_token"].as_str().unwrap_or("").to_string()
    };
    if document_id.is_empty() {
        return Err(fail(400, "Wiki 节点没有对应文档"));
    }
    let token = token.clone();
    let document_id_for_fetch = document_id.clone();
    let body = tokio::task::spawn_blocking(move || {
        get_json(
            &format!("https://open.feishu.cn/open-apis/docx/v1/documents/{document_id_for_fetch}"),
            &token,
        )
    })
    .await
    .map_err(|_| fail(500, "飞书文档读取失败"))??;
    let document = body.get("document").cloned().unwrap_or(body);
    Ok(json!({
        "document_id": document_id,
        "title": clip(document["title"].as_str().unwrap_or("飞书文档"), 200),
        "revision_id": document["revision_id"].as_str().unwrap_or("-1"),
    }))
}

fn fetch_blocks(document_id: &str, token: &str) -> Result<Vec<Value>, Fail> {
    if !valid_token(document_id) {
        return Err(fail(400, "飞书文档读取失败"));
    }
    let mut output = Vec::new();
    let mut page = String::new();
    for _ in 0..20 {
        let mut url = format!(
            "https://open.feishu.cn/open-apis/docx/v1/documents/{document_id}/blocks?page_size=500"
        );
        if !page.is_empty() {
            url.push_str("&page_token=");
            url.push_str(&encode(&page));
        }
        let data = get_json(&url, token)?;
        if let Some(items) = data["items"].as_array() {
            output.extend(items.iter().cloned());
        }
        if data["has_more"].as_bool() != Some(true) {
            break;
        }
        let next = data["page_token"].as_str().unwrap_or("");
        if next.is_empty() || next == page {
            break;
        }
        page = next.to_string();
    }
    Ok(output)
}

fn normalize_blocks(blocks: &[Value]) -> Value {
    let mut notices = Vec::new();
    let mut entries: Vec<Value> = Vec::new();
    let mut current: Option<usize> = None;
    let mut consumed = std::collections::HashSet::new();
    for (position, block) in blocks.iter().enumerate() {
        let block_id = block["block_id"].as_str().unwrap_or("");
        if !block_id.is_empty() && consumed.contains(block_id) {
            continue;
        }
        if let Some((table, used)) = table_item(block, blocks) {
            consumed.extend(used);
            push_item(&mut notices, &mut entries, current, table);
            continue;
        }
        let text = block_text(block);
        let assets = asset_tokens(block);
        if let Some((timestamp, start, end)) = find_time(&text) {
            let id = if block_id.is_empty() {
                short_hash(&format!("{position}:{timestamp}:{text}"))
            } else {
                block_id.to_string()
            };
            entries.push(json!({
                "id": id,
                "timestamp": timestamp,
                "day": &timestamp[..10],
                "time": &timestamp[11..16],
                "blocks": [],
            }));
            current = Some(entries.len() - 1);
            let remainder = format!("{}{}", &text[..start], &text[end..])
                .trim()
                .to_string();
            if !remainder.is_empty() || !assets.is_empty() {
                push_item(
                    &mut notices,
                    &mut entries,
                    current,
                    text_item(block_id, &remainder, assets),
                );
            }
            continue;
        }
        if text.is_empty() && assets.is_empty() {
            continue;
        }
        push_item(
            &mut notices,
            &mut entries,
            current,
            text_item(block_id, &text, assets),
        );
    }
    json!({"notices": notices, "entries": entries})
}

fn push_item(notices: &mut Vec<Value>, entries: &mut [Value], current: Option<usize>, item: Value) {
    if let Some(index) = current {
        entries[index]["blocks"].as_array_mut().unwrap().push(item);
    } else {
        notices.push(item);
    }
}

fn text_item(block_id: &str, text: &str, assets: Vec<Value>) -> Value {
    let mut item = json!({
        "type": if text.is_empty() { "asset" } else { "text" },
        "text": text,
        "block_id": block_id,
    });
    if let Some((speaker, reply, body)) = speaker_parts(text) {
        item["speaker"] = json!(speaker);
        item["reply_to"] = json!(reply);
        item["text"] = json!(body);
    }
    if !assets.is_empty() {
        item["assets"] = json!(assets);
    }
    item
}

fn speaker_parts(text: &str) -> Option<(String, String, String)> {
    let (left, right) = text.split_once('：').or_else(|| text.split_once(':'))?;
    if left.is_empty() || left.chars().count() > 40 || left.contains('\n') {
        return None;
    }
    let (speaker, reply) = left
        .split_once(" 回复 ")
        .map(|(a, b)| (a.trim(), b.trim()))
        .unwrap_or((left.trim(), ""));
    if speaker.is_empty() || speaker.chars().count() > 40 || reply.chars().count() > 40 {
        return None;
    }
    Some((
        speaker.to_string(),
        reply.to_string(),
        right.trim().to_string(),
    ))
}

fn find_time(text: &str) -> Option<(String, usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for index in 0..chars.len() {
        if let Some(found) = time_at(text, &chars, index) {
            return Some(found);
        }
    }
    None
}

fn time_at(text: &str, chars: &[(usize, char)], index: usize) -> Option<(String, usize, usize)> {
    if index > 0 && chars[index - 1].1.is_ascii_digit() {
        return None;
    }
    let mut cursor = index;
    let year = take_digits(chars, &mut cursor, 4)?;
    if !(2000..=2099).contains(&year) {
        return None;
    }
    take_sep(chars, &mut cursor)?;
    let month = take_digits(chars, &mut cursor, 2)?;
    take_sep(chars, &mut cursor)?;
    let day = take_digits(chars, &mut cursor, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if chars.get(cursor).is_some_and(|item| item.1 == '日') {
        cursor += 1;
    }
    if !chars.get(cursor).is_some_and(|item| item.1.is_whitespace()) {
        return None;
    }
    while chars.get(cursor).is_some_and(|item| item.1.is_whitespace()) {
        cursor += 1;
    }
    let hour = take_digits(chars, &mut cursor, 2)?;
    if chars.get(cursor).is_some_and(|item| item.1 == ':') {
        cursor += 1;
    } else {
        return None;
    }
    let minute = take_digits(chars, &mut cursor, 2)?;
    if !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || chars
            .get(cursor)
            .is_some_and(|item| item.1.is_ascii_digit())
    {
        return None;
    }
    let end = chars.get(cursor).map(|item| item.0).unwrap_or(text.len());
    Some((
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00+08:00"),
        chars[index].0,
        end,
    ))
}

fn take_digits(chars: &[(usize, char)], cursor: &mut usize, max: usize) -> Option<u32> {
    let start = *cursor;
    while *cursor < chars.len() && *cursor - start < max && chars[*cursor].1.is_ascii_digit() {
        *cursor += 1;
    }
    if *cursor == start {
        return None;
    }
    chars[start..*cursor]
        .iter()
        .map(|item| item.1)
        .collect::<String>()
        .parse()
        .ok()
}

fn take_sep(chars: &[(usize, char)], cursor: &mut usize) -> Option<()> {
    let sep = chars.get(*cursor)?.1;
    if matches!(sep, '-' | '/' | '.' | '年' | '月') {
        *cursor += 1;
        Some(())
    } else {
        None
    }
}

fn block_text(block: &Value) -> String {
    let mut parts = Vec::new();
    walk_text(block, &mut parts);
    if !parts.is_empty() {
        return parts.join("").trim().to_string();
    }
    for key in ["content", "title", "name"] {
        if let Some(text) = block.get(key).and_then(|item| item.as_str()) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
    }
    String::new()
}

fn walk_text(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| walk_text(item, parts)),
        Value::Object(map) => {
            if let Some(text) = map
                .get("text_run")
                .and_then(|item| item.get("content"))
                .and_then(|item| item.as_str())
            {
                parts.push(text.to_string());
            } else if let Some(text) = map
                .get("equation")
                .and_then(|item| item.get("content"))
                .and_then(|item| item.as_str())
            {
                parts.push(text.to_string());
            }
            map.values().for_each(|item| walk_text(item, parts));
        }
        _ => {}
    }
}

fn asset_tokens(block: &Value) -> Vec<Value> {
    let mut output = Vec::new();
    walk_assets(block, &mut output);
    output
}

fn walk_assets(value: &Value, output: &mut Vec<Value>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| walk_assets(item, output)),
        Value::Object(map) => {
            for (field, kind) in [("image", "image"), ("file", "file"), ("media", "file")] {
                let Some(media) = map.get(field).filter(|item| item.is_object()) else {
                    continue;
                };
                let token = media["token"]
                    .as_str()
                    .or_else(|| media["file_token"].as_str())
                    .unwrap_or("")
                    .trim();
                if valid_token(token) && !output.iter().any(|item| item["token"] == token) {
                    output.push(json!({"token": token, "name": clip(media["name"].as_str().or_else(|| media["file_name"].as_str()).unwrap_or(""), 200), "kind": kind}));
                }
            }
            map.values().for_each(|item| walk_assets(item, output));
        }
        _ => {}
    }
}

fn table_item(block: &Value, blocks: &[Value]) -> Option<(Value, Vec<String>)> {
    let table = block.get("table")?.as_object()?;
    let rows = table
        .get("property")
        .and_then(|item| item.get("row_size"))
        .and_then(|item| item.as_i64())? as usize;
    let columns = table
        .get("property")
        .and_then(|item| item.get("column_size"))
        .and_then(|item| item.as_i64())? as usize;
    let cells: Vec<String> = table
        .get("cells")?
        .as_array()?
        .iter()
        .filter_map(|item| item.as_str().map(|text| text.to_string()))
        .collect();
    if rows == 0 || columns == 0 || rows * columns != cells.len() {
        return None;
    }
    let mut used = cells.clone();
    let mut grid = Vec::new();
    for row in cells.chunks(columns) {
        let mut line = Vec::new();
        for cell_id in row {
            let cell = blocks
                .iter()
                .find(|item| item["block_id"].as_str() == Some(cell_id));
            let children: Vec<&Value> = cell
                .and_then(|item| item["children"].as_array())
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str())
                        .filter_map(|id| {
                            blocks
                                .iter()
                                .find(|item| item["block_id"].as_str() == Some(id))
                        })
                        .collect()
                })
                .unwrap_or_default();
            for child in &children {
                if let Some(id) = child["block_id"].as_str() {
                    used.push(id.to_string());
                }
            }
            let text = if children.is_empty() {
                cell.map(block_text).unwrap_or_default()
            } else {
                children
                    .iter()
                    .map(|item| block_text(item))
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let assets = if children.is_empty() {
                cell.map(asset_tokens).unwrap_or_default()
            } else {
                children
                    .iter()
                    .flat_map(|item| asset_tokens(item))
                    .collect()
            };
            let mut cell_value = json!({"text": text});
            if !assets.is_empty() {
                cell_value["assets"] = json!(assets);
            }
            line.push(cell_value);
        }
        grid.push(line);
    }
    Some((
        json!({"type": "table", "block_id": block["block_id"].as_str().unwrap_or(""), "rows": grid, "columns": columns}),
        used,
    ))
}

fn plain_text(title: &str, timeline: &Value) -> String {
    let mut lines = vec![title.trim().to_string()];
    for item in timeline["notices"].as_array().into_iter().flatten() {
        push_plain(&mut lines, item);
    }
    for entry in timeline["entries"].as_array().into_iter().flatten() {
        lines.push(entry["timestamp"].as_str().unwrap_or("").to_string());
        for item in entry["blocks"].as_array().into_iter().flatten() {
            push_plain(&mut lines, item);
        }
    }
    lines.retain(|line| !line.is_empty());
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn push_plain(lines: &mut Vec<String>, item: &Value) {
    if item["type"] == "table" {
        for row in item["rows"].as_array().into_iter().flatten() {
            let cells = row
                .as_array()
                .map(|cells| {
                    cells
                        .iter()
                        .map(|cell| cell["text"].as_str().unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .unwrap_or_default();
            lines.push(cells);
        }
        return;
    }
    let speaker = item["speaker"].as_str().unwrap_or("");
    let reply = item["reply_to"].as_str().unwrap_or("");
    let text = item["text"].as_str().unwrap_or("");
    if speaker.is_empty() {
        lines.push(text.to_string());
    } else if reply.is_empty() {
        lines.push(format!("{speaker}：{text}"));
    } else {
        lines.push(format!("{speaker} 回复 {reply}：{text}"));
    }
}

struct MediaFile {
    bytes: Vec<u8>,
    mime: String,
    name: String,
}

struct StoredAsset {
    name: String,
    filename: String,
    bytes: Vec<u8>,
}

fn publish_files(
    root: &Path,
    key_hash: &str,
    title: &str,
    timeline: &mut Value,
    fetch: &mut dyn FnMut(&str) -> Result<Option<MediaFile>, Fail>,
) -> Result<(String, String, String), Fail> {
    if !valid_archive_key(key_hash) {
        return Err(fail(400, "飞书文档读取失败"));
    }
    let stored = store_assets(timeline, fetch)?;
    let root = root
        .canonicalize()
        .map_err(|_| fail(503, "当前部署未挂载存储归档"))?;
    let digest = hex::encode(Sha256::digest(format!("{title}:{timeline}").as_bytes()));
    let version = root
        .join("feishu-documents")
        .join(key_hash)
        .join("versions")
        .join(&digest);
    if !version.exists() {
        let temp = version.with_file_name(format!(".{digest}.tmp"));
        fs::create_dir_all(temp.join("assets")).map_err(|_| fail(503, "知识库存储当前不可写"))?;
        for asset in &stored {
            fs::write(temp.join("assets").join(&asset.filename), &asset.bytes)
                .map_err(|_| fail(503, "知识库存储当前不可写"))?;
        }
        fs::write(temp.join("timeline.json"), timeline.to_string())
            .map_err(|_| fail(503, "知识库存储当前不可写"))?;
        fs::write(temp.join("content.txt"), plain_text(title, timeline))
            .map_err(|_| fail(503, "知识库存储当前不可写"))?;
        fs::rename(&temp, &version).map_err(|_| fail(503, "知识库存储当前不可写"))?;
    }
    let timeline_path = version
        .join("timeline.json")
        .canonicalize()
        .map_err(|_| fail(400, "飞书文档读取失败"))?;
    if !timeline_path.starts_with(&root) {
        return Err(fail(400, "飞书文档读取失败"));
    }
    let relative = |name: &str| format!("feishu-documents/{key_hash}/versions/{digest}/{name}");
    Ok((
        relative("timeline.json"),
        relative("content.txt"),
        relative("assets"),
    ))
}

fn store_assets(
    timeline: &mut Value,
    fetch: &mut dyn FnMut(&str) -> Result<Option<MediaFile>, Fail>,
) -> Result<Vec<StoredAsset>, Fail> {
    let mut tokens = Vec::new();
    collect_tokens(timeline, &mut tokens);
    let mut stored = Vec::new();
    let mut total = 0usize;
    for token in tokens {
        if stored.iter().any(|item: &StoredAsset| item.name == token) {
            continue;
        }
        let Some(file) = fetch(&token)? else {
            mark_asset(timeline, &token, "", "", "", true);
            continue;
        };
        if file.bytes.len() > 50 * 1024 * 1024 {
            return Err(fail(400, "飞书媒体文件超过 50 MB 限制"));
        }
        total += file.bytes.len();
        if total > 250 * 1024 * 1024 {
            return Err(fail(400, "飞书文档媒体总量超过 250 MB 限制"));
        }
        let id = hex::encode(Sha256::digest(&file.bytes));
        let filename = format!("{id}{}", media_extension(&file.mime, &file.name));
        mark_asset(timeline, &token, &id, &file.mime, &file.name, false);
        stored.push(StoredAsset {
            name: token,
            filename,
            bytes: file.bytes,
        });
    }
    Ok(stored)
}

fn collect_tokens(value: &Value, tokens: &mut Vec<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_tokens(item, tokens)),
        Value::Object(map) => {
            if let Some(assets) = map.get("assets").and_then(|item| item.as_array()) {
                for asset in assets {
                    let token = asset["token"].as_str().unwrap_or("");
                    if valid_token(token) && !tokens.iter().any(|item| item == token) {
                        tokens.push(token.to_string());
                    }
                }
            }
            map.values().for_each(|item| collect_tokens(item, tokens));
        }
        _ => {}
    }
}

fn mark_asset(value: &mut Value, token: &str, id: &str, mime: &str, name: &str, unavailable: bool) {
    match value {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| mark_asset(item, token, id, mime, name, unavailable)),
        Value::Object(map) => {
            if let Some(assets) = map.get_mut("assets").and_then(|item| item.as_array_mut()) {
                for asset in assets.iter_mut() {
                    if asset["token"].as_str() == Some(token) {
                        asset["id"] = json!(id);
                        if unavailable {
                            asset["unavailable"] = json!(true);
                        } else {
                            asset["mime"] = json!(mime);
                            if asset["name"].as_str().unwrap_or("").is_empty() && !name.is_empty() {
                                asset["name"] = json!(name);
                            }
                        }
                    }
                }
            }
            map.values_mut()
                .for_each(|item| mark_asset(item, token, id, mime, name, unavailable));
        }
        _ => {}
    }
}

fn media_extension(mime: &str, name: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/bmp" => ".bmp",
        _ => name_extension(name),
    }
}

fn name_extension(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => ".jpg",
        "png" => ".png",
        "gif" => ".gif",
        "webp" => ".webp",
        "pdf" => ".pdf",
        _ => ".bin",
    }
}

fn download_media(token: &str, access: &str) -> Result<Option<MediaFile>, Fail> {
    if !valid_token(token) {
        return Ok(None);
    }
    let response = ureq::get(&format!(
        "https://open.feishu.cn/open-apis/drive/v1/medias/{token}/download"
    ))
    .set("Authorization", &format!("Bearer {access}"))
    .timeout(std::time::Duration::from_secs(30))
    .call();
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => return Err(fail(400, "飞书授权已失效")),
        Err(_) => return Ok(None),
    };
    let mime = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    let disposition = response
        .header("content-disposition")
        .unwrap_or("")
        .to_string();
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(50 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| fail(400, "飞书媒体下载失败"))?;
    if bytes.len() > 50 * 1024 * 1024 {
        return Err(fail(400, "飞书媒体文件超过 50 MB 限制"));
    }
    Ok(Some(MediaFile {
        bytes,
        mime,
        name: disposition_name(&disposition),
    }))
}

fn disposition_name(header: &str) -> String {
    let Some(start) = header.to_ascii_lowercase().find("filename=") else {
        return String::new();
    };
    let raw = header[start + "filename=".len()..].trim().trim_matches('"');
    clip(raw.split(';').next().unwrap_or("").trim(), 200)
}

fn short_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))[..20].to_string()
}

fn archive_root() -> Result<PathBuf, Fail> {
    crate::ima_admin::archive_root().map_err(|_| fail(503, "当前部署未挂载存储归档"))
}

struct Token {
    access: String,
    refresh: String,
    expires_in: i64,
    refresh_expires_in: i64,
}

struct Cfg {
    configured: bool,
    app_id: String,
    redirect: String,
    scopes: String,
    interval: i64,
    public: Value,
}

async fn config(db: &Db) -> Result<Cfg, Fail> {
    let app_id = first(db, "feishu_docs_app_id", "FEISHU_DOCS_APP_ID").await?;
    let secret = secret(db).await?;
    let redirect = first(db, "feishu_docs_redirect_uri", "FEISHU_DOCS_REDIRECT_URI").await?;
    let scopes = {
        let stored = first(db, "feishu_docs_scopes", "FEISHU_DOCS_SCOPES").await?;
        if stored.is_empty() {
            DEFAULT_SCOPES.to_string()
        } else {
            stored
        }
    };
    let interval = first(db, "feishu_docs_interval_seconds", "FEISHU_DOCS_INTERVAL")
        .await?
        .parse::<i64>()
        .unwrap_or(60)
        .clamp(15, 86400);
    let path_ok = redirect
        .split('?')
        .next()
        .unwrap_or("")
        .ends_with("/api/admin/feishu-documents/oauth/callback");
    let stored = text(db, "feishu_docs_app_id").await?;
    let source = if !stored.is_empty() {
        "db"
    } else if !app_id.is_empty() && !secret.is_empty() && !redirect.is_empty() {
        "env"
    } else {
        ""
    };
    let configured = !app_id.is_empty() && !secret.is_empty() && !redirect.is_empty();
    let public = json!({
        "app_id": app_id,
        "app_secret_set": !secret.is_empty(),
        "redirect_uri": redirect,
        "redirect_path_ok": path_ok,
        "scopes": scopes,
        "interval_seconds": interval,
        "config_source": source,
    });
    Ok(Cfg {
        configured,
        app_id,
        redirect,
        scopes,
        interval,
        public,
    })
}

async fn secret(db: &Db) -> Result<String, Fail> {
    let stored = text(db, "feishu_docs_app_secret").await?;
    if !stored.is_empty() {
        let key = credential_key().ok_or(fail(400, "未配置 FEISHU_CREDENTIAL_KEY"))?;
        return open_app_secret(&key, &stored).map_err(|_| fail(400, "App Secret 无法解密"));
    }
    Ok(std::env::var("FEISHU_DOCS_APP_SECRET").unwrap_or_default())
}

fn credential_state(access_expires: i64, refresh_expires: i64, now: i64) -> &'static str {
    if access_expires > now + 60 {
        "access"
    } else if refresh_expires == 0 || refresh_expires > now {
        "refresh"
    } else {
        "reauth"
    }
}

async fn access_token(db: &Db) -> Result<String, Fail> {
    let row = sqlx::query(
        "SELECT access_token, refresh_token, expires_at, refresh_expires_at FROM feishu_oauth_credentials WHERE id = 1",
    )
    .fetch_optional(db.pool())
    .await
    .map_err(|_| fail(400, "飞书授权失败"))?;
    let Some(row) = row else {
        return Err(fail(400, "请先授权飞书文档"));
    };
    let key = credential_key().ok_or(fail(400, "未配置 FEISHU_CREDENTIAL_KEY"))?;
    let expires: i64 = row.get("expires_at");
    let refresh_expires: i64 = row.get("refresh_expires_at");
    let current = now() as i64;
    if credential_state(expires, refresh_expires, current) == "access" {
        return open_app_secret(&key, &row.get::<String, _>("access_token"))
            .map_err(|_| fail(400, "飞书授权无法解密"));
    }
    if credential_state(expires, refresh_expires, current) != "refresh" {
        return Err(fail(400, "飞书授权已过期，请重新授权"));
    }
    let refresh = open_app_secret(&key, &row.get::<String, _>("refresh_token"))
        .map_err(|_| fail(400, "飞书授权无法解密"))?;
    if refresh.is_empty() {
        return Err(fail(400, "飞书授权已过期，请重新授权"));
    }
    let cfg = config(db).await?;
    let app_secret = secret(db).await?;
    let app_id = cfg.app_id.clone();
    let app_secret_owned = app_secret.clone();
    let refresh_owned = refresh.clone();
    let body = tokio::task::spawn_blocking(move || {
        exchange_refresh(&app_id, &app_secret_owned, &refresh_owned)
    })
    .await
    .map_err(|_| fail(400, "飞书授权失败"))??;
    let parsed = parse_token(&body)?;
    if parsed.refresh.is_empty() {
        return Err(fail(400, "飞书授权失败"));
    }
    save_token(db, &body).await?;
    Ok(parsed.access)
}

pub fn spawn_sync(db: Db) {
    tokio::spawn(async move {
        loop {
            let wait = match config(&db).await {
                Ok(cfg) => cfg.interval.max(15) as u64,
                Err(_) => 60,
            };
            let ids = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM feishu_document_sources WHERE enabled = 1 AND deleted_at IS NULL ORDER BY id",
            )
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
            for id in ids {
                if let Err(err) = sync_source(&db, id).await {
                    tracing::warn!(source = id, "飞书文档同步失败: {}", err.detail);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        }
    });
}

async fn credential_live(db: &Db) -> Result<bool, Fail> {
    let row = sqlx::query("SELECT refresh_expires_at FROM feishu_oauth_credentials WHERE id = 1")
        .fetch_optional(db.pool())
        .await
        .map_err(|_| fail(400, "读取飞书授权失败"))?;
    Ok(row.is_some_and(|row| {
        let expires: i64 = row.get("refresh_expires_at");
        expires == 0 || expires > now() as i64
    }))
}

async fn list_sources(db: &Db, interval: i64) -> Result<Vec<Value>, Fail> {
    let rows = sqlx::query(
        "SELECT id, source_type, canonical_url, group_id, media_id, title, display_name, revision_id, entry_count, enabled, display_mode, sync_status, last_checked_at, last_success_at, last_error
         FROM feishu_document_sources WHERE deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(db.pool())
    .await
    .map_err(|_| fail(400, "读取飞书文档失败"))?;
    Ok(rows
        .iter()
        .map(|row| public_source(row, interval))
        .collect())
}

async fn source_json(db: &Db, id: i64) -> Result<Value, Fail> {
    let interval = config(db).await?.interval;
    let row = source_row(db, id).await?;
    Ok(public_source(&row, interval))
}

async fn source_row(db: &Db, id: i64) -> Result<sqlx::sqlite::SqliteRow, Fail> {
    sqlx::query(
        "SELECT id, source_type, canonical_url, group_id, media_id, title, display_name, revision_id, entry_count, enabled, display_mode, sync_status, last_checked_at, last_success_at, last_error, deleted_at
         FROM feishu_document_sources WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await
    .map_err(|_| fail(400, "读取飞书文档失败"))?
    .filter(|row| row.get::<Option<String>, _>("deleted_at").is_none())
    .ok_or(fail(404, "飞书文档来源不存在"))
}

fn public_source(row: &sqlx::sqlite::SqliteRow, interval: i64) -> Value {
    let enabled = row.get::<i64, _>("enabled") != 0;
    let last_checked = row.get::<String, _>("last_checked_at");
    let next = if enabled && !last_checked.is_empty() {
        next_check(&last_checked, interval)
    } else {
        String::new()
    };
    let error = row.get::<String, _>("last_error");
    let title = row.get::<String, _>("title");
    let title = if title.is_empty() {
        "待首次同步".to_string()
    } else {
        title
    };
    json!({
        "id": row.get::<i64, _>("id"),
        "source_type": row.get::<String, _>("source_type"),
        "canonical_url": row.get::<String, _>("canonical_url"),
        "group_id": row.get::<String, _>("group_id"),
        "media_id": row.get::<String, _>("media_id"),
        "title": title,
        "display_name": row.get::<String, _>("display_name"),
        "revision_id": row.get::<String, _>("revision_id"),
        "entry_count": row.get::<i64, _>("entry_count"),
        "enabled": enabled,
        "display_mode": row.get::<String, _>("display_mode"),
        "sync_status": row.get::<String, _>("sync_status"),
        "last_checked_at": last_checked,
        "last_success_at": row.get::<String, _>("last_success_at"),
        "next_check_at": next,
        "last_error": error.chars().take(300).collect::<String>(),
    })
}

fn parse_token(text: &str) -> Result<Token, Fail> {
    let value: Value = serde_json::from_str(text).map_err(|_| fail(400, "飞书授权失败"))?;
    let data = value.get("data").cloned().unwrap_or(value);
    let code = data.get("code").and_then(|item| item.as_i64()).unwrap_or(0);
    if code != 0 {
        return Err(fail(400, "飞书授权失败"));
    }
    let access = data["access_token"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if access.is_empty() {
        return Err(fail(400, "飞书授权返回缺少访问令牌"));
    }
    Ok(Token {
        access,
        refresh: data["refresh_token"].as_str().unwrap_or("").to_string(),
        expires_in: data["expires_in"]
            .as_i64()
            .unwrap_or(7200)
            .clamp(60, 86_400),
        refresh_expires_in: data["refresh_expires_in"]
            .as_i64()
            .unwrap_or(2_592_000)
            .clamp(60, 31_536_000),
    })
}

fn get_json(url: &str, token: &str) -> Result<Value, Fail> {
    let response = ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(20))
        .call();
    let text = match response {
        Ok(response) => response
            .into_string()
            .map_err(|_| fail(400, "飞书文档读取失败"))?,
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            let code = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value.get("code").and_then(|item| item.as_i64()));
            tracing::warn!(status, code, "飞书文档 HTTP 失败");
            return Err(fail(400, "飞书文档读取失败"));
        }
        Err(_) => return Err(fail(400, "飞书文档读取失败")),
    };
    let value: Value = serde_json::from_str(&text).map_err(|_| fail(400, "飞书返回了无效数据"))?;
    let code = value
        .get("code")
        .and_then(|item| item.as_i64())
        .unwrap_or(0);
    if matches!(code, 99991661 | 99991663 | 99991668) {
        return Err(fail(400, "飞书授权已失效"));
    }
    if code != 0 {
        tracing::warn!(code, "飞书文档接口拒绝");
        return Err(fail(400, "飞书文档读取失败"));
    }
    Ok(value.get("data").cloned().unwrap_or(value))
}

async fn text(db: &Db, key: &str) -> Result<String, Fail> {
    db.setting(key)
        .await
        .map_err(|_| fail(400, "读取飞书文档配置失败"))
        .map(|value| value.unwrap_or_default())
}

async fn first(db: &Db, key: &str, env: &str) -> Result<String, Fail> {
    let stored = text(db, key).await?;
    if stored.is_empty() {
        Ok(std::env::var(env).unwrap_or_default())
    } else {
        Ok(stored)
    }
}

fn trusted_host(host: &str) -> bool {
    ["feishu.cn", "larksuite.com"]
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

fn valid_archive_key(value: &str) -> bool {
    matches!(value.len(), 20 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_token(token: &str) -> bool {
    (8..=128).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn random_token(n: usize) -> Result<String, Fail> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|_| fail(400, "无法开始飞书授权"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

fn encode(value: &str) -> String {
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn clip(value: &str, max: usize) -> String {
    value.trim().chars().take(max).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn chrono_like_now() -> String {
    now().to_string()
}

fn next_check(last: &str, interval: i64) -> String {
    let Ok(seconds) = last.parse::<i64>() else {
        return String::new();
    };
    (seconds + interval).to_string()
}

fn fail(status: u16, detail: &'static str) -> Fail {
    Fail { status, detail }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (Db, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "vpush-feishu-admin-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        (db, path)
    }

    #[test]
    fn parse_accepts_wiki_and_rejects_other_hosts() {
        let parsed =
            parse_url("https://example.feishu.cn/wiki/doxcnABCDEFG1234?from=share").unwrap();
        assert_eq!(parsed.source_type, "wiki");
        assert!(parsed.group_id.starts_with("feishu-"));
        assert!(parse_url("https://example.com/wiki/doxcnABCDEFG1234").is_err());
        assert!(parse_url("http://example.feishu.cn/docx/doxcnABCDEFG1234").is_err());
    }

    #[tokio::test]
    async fn config_hides_secret_and_source_can_be_renamed() {
        let _env_lock = crate::feishu_personal::TEST_ENV_LOCK
            .get_or_init(|| async { tokio::sync::Mutex::new(()) })
            .await
            .lock()
            .await;
        let (db, path) = db().await;
        std::env::set_var("FEISHU_CREDENTIAL_KEY", URL_SAFE.encode([7u8; 32]));
        let saved = save_config(
            &db,
            Save {
                app_id: Some("cli_test"),
                app_secret: Some("secret-value"),
                redirect_uri: Some(
                    "https://vpush.example/api/admin/feishu-documents/oauth/callback",
                ),
                scopes: Some("wiki:node:read docx:document:readonly"),
                interval_seconds: Some(60),
            },
        )
        .await
        .unwrap();
        let body = saved.to_string();
        assert!(!body.contains("secret-value"));
        assert_eq!(saved["config"]["app_secret_set"], true);
        assert_eq!(saved["config"]["redirect_path_ok"], true);
        let source = add_source(&db, "https://example.feishu.cn/docx/doxcnABCDEFG1234")
            .await
            .unwrap();
        assert_eq!(source["sync_status"], "pending");
        let id = source["id"].as_i64().unwrap();
        let renamed = update_source(&db, id, None, Some("document"), Some("投研纪要"))
            .await
            .unwrap();
        assert_eq!(renamed["display_name"], "投研纪要");
        assert_eq!(renamed["display_mode"], "document");
        assert!(note_sync(&db, id, None).await.is_err());
        remove_source(&db, id).await.unwrap();
        assert!(update_source(&db, id, Some(false), None, None)
            .await
            .is_err());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn callback_rejects_a_mismatched_cookie() {
        let _env_lock = crate::feishu_personal::TEST_ENV_LOCK
            .get_or_init(|| async { tokio::sync::Mutex::new(()) })
            .await
            .lock()
            .await;
        let (db, path) = db().await;
        std::env::set_var("FEISHU_CREDENTIAL_KEY", URL_SAFE.encode([9u8; 32]));
        save_config(
            &db,
            Save {
                app_id: Some("cli_test"),
                app_secret: Some("secret-value"),
                redirect_uri: Some(
                    "https://vpush.example/api/admin/feishu-documents/oauth/callback",
                ),
                scopes: None,
                interval_seconds: None,
            },
        )
        .await
        .unwrap();
        let (url, hash) = begin_oauth(&db, 1).await.unwrap();
        assert!(url.contains("client_id=cli_test"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(take_session(&db, "wrong-state", &hash).await.is_err());
        assert!(!credential_live(&db).await.unwrap());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn migrated_document_keys_can_be_published() {
        assert!(valid_archive_key(&"a".repeat(20)));
        assert!(valid_archive_key(&"ab".repeat(32)));
        assert!(!valid_archive_key(&"g".repeat(64)));
        assert!(!valid_archive_key(&"../".repeat(8)));
    }

    #[test]
    fn expired_access_refreshes_until_the_refresh_token_expires() {
        assert_eq!(credential_state(200, 500, 100), "access");
        assert_eq!(credential_state(150, 500, 100), "refresh");
        assert_eq!(credential_state(100, 0, 100), "refresh");
        assert_eq!(credential_state(100, 100, 100), "reauth");
    }

    #[test]
    fn dated_text_becomes_a_timeline_entry() {
        let blocks = vec![
            json!({"block_id": "n1", "text": {"elements": [{"text_run": {"content": "开场说明"}}]}}),
            json!({"block_id": "e1", "text": {"elements": [{"text_run": {"content": "2024-03-02 09:30 张三：早上好"}}]}}),
        ];
        let timeline = normalize_blocks(&blocks);
        assert_eq!(timeline["notices"][0]["text"], "开场说明");
        assert_eq!(timeline["entries"][0]["day"], "2024-03-02");
        assert_eq!(timeline["entries"][0]["time"], "09:30");
        assert_eq!(timeline["entries"][0]["blocks"][0]["speaker"], "张三");
        assert_eq!(timeline["entries"][0]["blocks"][0]["text"], "早上好");
    }

    #[tokio::test]
    async fn published_timeline_can_be_read() {
        let (db, path) = db().await;
        let root = std::env::temp_dir().join(format!(
            "vpush-feishu-archive-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("IMA_ARCHIVE_ROOT", &root);
        let source = add_source(&db, "https://example.feishu.cn/docx/doxcnABCDEFG1234")
            .await
            .unwrap();
        let id = source["id"].as_i64().unwrap();
        let hash: String =
            sqlx::query_scalar("SELECT source_key_hash FROM feishu_document_sources WHERE id = ?")
                .bind(id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let blocks = vec![
            json!({"block_id": "e1", "text": {"elements": [{"text_run": {"content": "2024-03-02 09:30 纪要正文"}}]}}),
        ];
        let mut timeline = normalize_blocks(&blocks);
        let (timeline_path, txt_path, _) =
            publish_files(&root, &hash, "投研纪要", &mut timeline, &mut |_| {
                Ok(None)
            })
            .unwrap();
        sqlx::query("UPDATE feishu_document_sources SET title = '投研纪要', timeline_path = ?, sync_status = 'succeeded' WHERE id = ?")
            .bind(&timeline_path)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        let page = crate::feishu_docs::timeline_all(&db, &root, 0, true, "", "latest", None, "")
            .await
            .unwrap();
        assert_eq!(page["entries"][0]["blocks"][0]["text"], "纪要正文");
        assert!(std::fs::read_to_string(root.join(&txt_path))
            .unwrap()
            .contains("纪要正文"));
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn downloaded_image_is_named_by_its_hash() {
        let root = std::env::temp_dir().join(format!(
            "vpush-feishu-asset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut timeline = json!({
            "notices": [{"type": "asset", "text": "", "assets": [{"token": "boxcnImageToken", "name": "", "kind": "image"}]}],
            "entries": []
        });
        let (_, _, asset_root) = publish_files(
            &root,
            "abcdef0123456789abcd",
            "纪要",
            &mut timeline,
            &mut |_| {
                Ok(Some(MediaFile {
                    bytes: b"png-bytes".to_vec(),
                    mime: "image/png".to_string(),
                    name: "图表.png".to_string(),
                }))
            },
        )
        .unwrap();
        let id = timeline["notices"][0]["assets"][0]["id"].as_str().unwrap();
        assert_eq!(id.len(), 64);
        let file = root.join(&asset_root).join(format!("{id}.png"));
        assert_eq!(std::fs::read(&file).unwrap(), b"png-bytes");
        assert_eq!(timeline["notices"][0]["assets"][0]["name"], "图表.png");
        let _ = std::fs::remove_dir_all(root);
    }
}
