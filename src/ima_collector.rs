//! IMA 采集配置。保存 UID、令牌和知识库挂载；真正的腾讯侧下载还没接上。

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::Db;
use crate::ima_client;

const UID_KEY: &str = "ima_pure_uid";
const TOKEN_KEY: &str = "ima_pure_refresh_token";
const KB_KEY: &str = "ima_pure_knowledge_base_id";
const ROOT_KEY: &str = "ima_pure_root_folder_id";
const GROUPS_KEY: &str = "ima_pure_groups";
const INTERVAL_KEY: &str = "ima_pure_interval_seconds";
const STARTED_KEY: &str = "ima_pure_last_started_at";
const FAIL_STREAK_KEY: &str = "ima_pure_fail_streak";
const FINISHED_KEY: &str = "ima_pure_last_finished_at";
const RESULT_KEY: &str = "ima_pure_last_result";
const DISCOVERY_KEY: &str = "ima_pure_discovery";

#[derive(Debug)]
pub enum CollectorError {
    Bad(&'static str),
    NotFound(&'static str),
    Conflict(&'static str),
    TooSoon,
    Unavailable(&'static str),
    Upstream(String),
}

#[derive(Deserialize)]
pub struct SaveBody {
    uid: Option<String>,
    refresh_token: Option<String>,
    knowledge_base_id: Option<String>,
    root_folder_id: Option<String>,
    interval_seconds: Option<i64>,
    groups: Option<Vec<GroupIn>>,
}

#[derive(Deserialize)]
struct GroupIn {
    id: Option<String>,
    name: String,
    knowledge_base_id: String,
    root_folder_id: String,
    enabled: Option<bool>,
    folder_ids: Option<Vec<Value>>,
    interval_seconds: Option<i64>,
}

#[derive(Deserialize)]
pub struct SyncBody {
    /// None 或空串 = 同步所有已启用且挂载了的库（维护循环走这条）。
    pub group_id: Option<String>,
}

#[derive(Clone)]
struct Group {
    id: String,
    name: String,
    knowledge_base_id: String,
    root_folder_id: String,
    enabled: bool,
    source: String,
    folder_ids: Option<Vec<String>>,
    interval_seconds: i64,
}

pub async fn status(db: &Db) -> Result<Value, String> {
    let cfg = load(db).await?;
    let mut groups = Vec::new();
    for group in &cfg.groups {
        let mut row = group_public(group);
        let names = db
            .ima_kb_acl_usernames(&group.id)
            .await
            .map_err(|err| err.to_string())?;
        row["acl_usernames"] = json!(names);
        groups.push(row);
    }
    let mut config = config_public(&cfg);
    config["groups"] = json!(groups);
    let documents = db
        .ima_document_count()
        .await
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "config": config,
        "running": RUNNING.load(std::sync::atomic::Ordering::SeqCst),
        "next_run_at": 0,
        "last_started_at": text(db, STARTED_KEY).await?,
        "last_finished_at": text(db, FINISHED_KEY).await?,
        "last_result": json_setting(db, RESULT_KEY).await?,
        "discovery": discovery(db).await?,
        "documents": documents,
        "progress": null,
        "index": {"status": "ready"},
        "full_text_index": {"enabled": false, "ready": false, "documents": 0, "last_sync_at": "", "error": ""}
    }))
}

/// 给非管理员开通当前启用的研报库。名单与管理端用户弹窗一致，写入走 `Db::set_ima_kb_acl_for_user`。
/// 失败只记日志。`WEB_GRANT_DEFAULT_IMA_KB` 为 `0`、`false`、`False` 时跳过。
pub async fn grant_default_libraries(db: &Db, user_id: i64) {
    if !default_library_grant_enabled() {
        return;
    }
    if let Err(err) = try_grant_default_libraries(db, user_id).await {
        tracing::warn!(user_id, "新用户研报库开通失败: {err}");
    }
}

fn default_library_grant_enabled() -> bool {
    !matches!(
        std::env::var("WEB_GRANT_DEFAULT_IMA_KB").ok().as_deref(),
        Some("0" | "false" | "False")
    )
}

async fn try_grant_default_libraries(db: &Db, user_id: i64) -> Result<(), String> {
    let user = db
        .user_by_id(user_id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "用户不存在".to_string())?;
    if user.is_admin {
        return Ok(());
    }
    let ids = enabled_research_group_ids(db).await?;
    if ids.is_empty() {
        return Ok(());
    }
    db.set_ima_kb_acl_for_user(user_id, &ids)
        .await
        .map_err(|err| err.to_string())?;
    tracing::info!(
        user_id,
        groups = %ids.join(","),
        "新用户已开通启用中的研报库"
    );
    Ok(())
}

async fn enabled_research_group_ids(db: &Db) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for id in collector_enabled_ids(db).await? {
        remember_group(&mut ids, &id);
    }
    for id in local_enabled_ids(db).await? {
        remember_group(&mut ids, &id);
    }
    Ok(ids)
}

async fn collector_enabled_ids(db: &Db) -> Result<Vec<String>, String> {
    let status = status(db).await?;
    let mut ids = Vec::new();
    for group in status["config"]["groups"].as_array().into_iter().flatten() {
        let Some(id) = group["id"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if id.starts_with("feishu-") || group["enabled"] == false {
            continue;
        }
        if !grantable_group_id(id) {
            tracing::warn!(group_id = id, "跳过格式无效的研报库");
            continue;
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

async fn local_enabled_ids(db: &Db) -> Result<Vec<String>, String> {
    let payload = crate::ima_admin::libraries(db)
        .await
        .map_err(|err| err.detail)?;
    let mut ids = Vec::new();
    for lib in payload["libraries"].as_array().into_iter().flatten() {
        let Some(id) = lib["group_id"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let name = lib["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .or_else(|| {
                lib["slug"]
                    .as_str()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
            })
            .unwrap_or(id);
        let enabled =
            lib["enabled"] == true && lib["error"].as_str().is_none_or(|text| text.is_empty());
        if name.is_empty() || id.starts_with("feishu-") || !enabled {
            continue;
        }
        if !grantable_group_id(id) {
            tracing::warn!(group_id = id, "跳过格式无效的研报库");
            continue;
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

fn remember_group(ids: &mut Vec<String>, id: &str) {
    if !ids.iter().any(|item| item == id) {
        ids.push(id.to_string());
    }
}

fn grantable_group_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-'))
}

#[cfg(test)]
pub(crate) static GRANT_ENV_LOCK: tokio::sync::OnceCell<tokio::sync::Mutex<()>> =
    tokio::sync::OnceCell::const_new();

#[cfg(test)]
pub(crate) async fn lock_grant_env() -> tokio::sync::MutexGuard<'static, ()> {
    GRANT_ENV_LOCK
        .get_or_init(|| async { tokio::sync::Mutex::new(()) })
        .await
        .lock()
        .await
}

pub async fn save(db: &Db, body: &SaveBody) -> Result<Value, CollectorError> {
    let current = load(db)
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 配置保存失败"))?;
    let mut updates: Vec<(&str, String)> = Vec::new();
    if let Some(uid) = &body.uid {
        let uid = uid.trim();
        if !ident(uid, 64, false) {
            return Err(CollectorError::Bad("IMA UID 格式无效"));
        }
        updates.push((UID_KEY, uid.to_string()));
    }
    if let Some(token) = body
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if token.len() > 4096 {
            return Err(CollectorError::Bad("Refresh Token 过长"));
        }
        updates.push((TOKEN_KEY, token.to_string()));
    }
    if let Some(kb) = &body.knowledge_base_id {
        let kb = kb.trim();
        if !ident(kb, 64, false) {
            return Err(CollectorError::Bad("知识库 ID 格式无效"));
        }
        updates.push((KB_KEY, kb.to_string()));
    }
    if let Some(root) = &body.root_folder_id {
        let root = root.trim();
        if !ident(root, 128, false) {
            return Err(CollectorError::Bad("根文件夹 ID 格式无效"));
        }
        updates.push((ROOT_KEY, root.to_string()));
    }
    if let Some(interval) = body.interval_seconds {
        if !(1800..=604_800).contains(&interval) {
            return Err(CollectorError::Bad("同步间隔须在 1800–604800 秒"));
        }
        updates.push((INTERVAL_KEY, interval.to_string()));
    }
    if let Some(groups) = &body.groups {
        updates.push((
            GROUPS_KEY,
            normalize_groups(groups, &current.groups)?.to_string(),
        ));
    }
    for (key, value) in updates {
        db.set_setting(key, &value)
            .await
            .map_err(|_| CollectorError::Unavailable("IMA 配置保存失败"))?;
    }
    status(db)
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 配置保存失败"))
}

pub async fn discover(db: &Db) -> Result<Value, CollectorError> {
    let cfg = load(db)
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 知识库发现尚未接入"))?;
    if cfg.uid.is_empty() || cfg.refresh_token.is_empty() {
        return Err(CollectorError::Bad("请先配置 IMA UID 和 Refresh Token"));
    }
    Err(CollectorError::Unavailable("IMA 知识库发现尚未接入"))
}

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Held for the whole sync; the maintenance loop and manual triggers share it,
/// so two runs never download into the same files concurrently.
struct RunGuard;

impl RunGuard {
    fn acquire() -> Option<Self> {
        // Never build a RunGuard on failure: dropping it would clear the flag.
        if RUNNING
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            Some(RunGuard)
        } else {
            None
        }
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn plan(db: &Db, body: &SyncBody) -> Result<(Cfg, Vec<Group>, bool), CollectorError> {
    let cfg = load(db)
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 文档下载尚未接入"))?;
    if cfg.uid.is_empty() || cfg.refresh_token.is_empty() {
        return Err(CollectorError::Bad("请先配置 IMA UID 和 Refresh Token"));
    }
    let group_id = body.group_id.as_deref().unwrap_or("").trim();
    if !group_id.is_empty() {
        let Some(group) = cfg.groups.iter().find(|group| group.id == group_id) else {
            return Err(CollectorError::NotFound("知识库不存在"));
        };
        if mount_ids(group).is_empty() {
            return Err(CollectorError::Conflict("请先挂载该知识库"));
        }
    }
    let targets: Vec<Group> = cfg
        .groups
        .iter()
        .filter(|group| {
            if group_id.is_empty() {
                group.enabled && !mount_ids(group).is_empty()
            } else {
                group.id == group_id
            }
        })
        .cloned()
        .collect();
    Ok((cfg, targets, group_id.is_empty()))
}

/// Scheduled path (maintenance loop): runs inline and honours the interval.
pub async fn sync(db: &Db, body: &SyncBody) -> Result<Value, CollectorError> {
    let (cfg, targets, scheduled) = plan(db, body).await?;
    let Some(guard) = RunGuard::acquire() else {
        return Err(CollectorError::TooSoon);
    };
    if scheduled
        && too_soon(&cfg, db)
            .await
            .map_err(|_| CollectorError::Unavailable("IMA 文档下载尚未接入"))?
            .is_some()
    {
        return Err(CollectorError::TooSoon);
    }
    let result = run(db, &cfg, &targets).await;
    drop(guard);
    result
}

/// Manual trigger from the admin page: validates, then runs in the background
/// so a proxy timeout or closed tab cannot cancel the sync half-way.
pub async fn start_sync(db: &Db, body: &SyncBody) -> Result<Value, CollectorError> {
    let (cfg, targets, scheduled) = plan(db, body).await?;
    let Some(guard) = RunGuard::acquire() else {
        return Ok(json!({"ok": true, "status": "already_running"}));
    };
    if scheduled
        && too_soon(&cfg, db)
            .await
            .map_err(|_| CollectorError::Unavailable("IMA 文档下载尚未接入"))?
            .is_some()
    {
        return Err(CollectorError::TooSoon);
    }
    let db = db.clone();
    tokio::spawn(async move {
        let _guard = guard;
        if let Some(Err(err)) = crate::supervise::catch("ima_sync", run(&db, &cfg, &targets)).await
        {
            tracing::warn!("IMA 手动同步失败: {err:?}");
        }
    });
    Ok(json!({"ok": true, "status": "started"}))
}

async fn run(db: &Db, cfg: &Cfg, targets: &[Group]) -> Result<Value, CollectorError> {
    let exit = crate::proxy_admin::acquire(db, "ima")
        .await
        .map_err(|err| match err.as_str() {
            "代理池为空" => CollectorError::Bad("代理池为空"),
            "指定代理不存在" => CollectorError::Bad("指定代理不存在"),
            "指定代理已过期" => CollectorError::Bad("指定代理已过期"),
            _ => CollectorError::Unavailable("IMA 代理不可用"),
        })?;
    let proxy_id = exit.as_ref().map(|item| item.id);
    let http = ima_client::Live::proxied(exit.map(|item| item.url));
    let root = std::env::var("IMA_ARCHIVE_ROOT")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let result = sync_groups(db, cfg, targets, &http, root.as_deref()).await;
    crate::proxy_admin::note(db, proxy_id, result.is_ok(), "IMA 同步失败").await;
    result
}

pub async fn folders(db: &Db, group_id: &str, parent_id: &str) -> Result<Value, CollectorError> {
    if !ident(group_id, 128, true) {
        return Err(CollectorError::NotFound("知识库不存在"));
    }
    let parent = parent_id.trim();
    if !parent.is_empty() && !ident(parent, 128, true) {
        return Err(CollectorError::Bad("父文件夹 ID 格式无效"));
    }
    let cfg = load(db)
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 文档服务未启用"))?;
    let Some(group) = cfg
        .groups
        .iter()
        .find(|group| group.id == group_id)
        .cloned()
    else {
        return Err(CollectorError::NotFound("知识库不存在"));
    };
    let actual = if parent.is_empty() {
        group.root_folder_id.trim().to_string()
    } else {
        parent.to_string()
    };
    if !ident(&actual, 128, true) {
        return Err(CollectorError::Bad("根文件夹 ID 格式无效"));
    }
    if cfg.uid.is_empty() || cfg.refresh_token.is_empty() {
        return Err(CollectorError::Unavailable("IMA 文档服务未启用"));
    }
    let exit = crate::proxy_admin::acquire(db, "ima")
        .await
        .map_err(|err| match err.as_str() {
            "代理池为空" => CollectorError::Bad("代理池为空"),
            "指定代理不存在" => CollectorError::Bad("指定代理不存在"),
            "指定代理已过期" => CollectorError::Bad("指定代理已过期"),
            _ => CollectorError::Unavailable("IMA 代理不可用"),
        })?;
    let http = ima_client::Live::proxied(exit.map(|item| item.url));
    list_group_folders(&cfg, &group, &actual, &http).await
}

async fn list_group_folders(
    cfg: &Cfg,
    group: &Group,
    parent_id: &str,
    http: &impl ima_client::Transport,
) -> Result<Value, CollectorError> {
    let session = ima_client::refresh(http, &ima_client::base(), &cfg.uid, &cfg.refresh_token)
        .await
        .map_err(|err| {
            CollectorError::Upstream(format!("IMA 文件夹读取失败: {}", safe_ima(&err)))
        })?;
    let items = ima_client::list_folders(
        http,
        &ima_client::base(),
        &session,
        &group.knowledge_base_id,
        parent_id,
    )
    .await
    .map_err(|err| CollectorError::Upstream(format!("IMA 文件夹读取失败: {}", safe_ima(&err))))?;
    Ok(json!({"group_id": group.id, "parent_id": parent_id, "items": items}))
}

fn safe_ima(err: &str) -> String {
    let text: String = err.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(80).collect();
    if text.is_empty() {
        "未知错误".into()
    } else {
        text
    }
}

async fn sync_groups(
    db: &Db,
    cfg: &Cfg,
    targets: &[Group],
    http: &impl ima_client::Transport,
    root: Option<&std::path::Path>,
) -> Result<Value, CollectorError> {
    let now = now_secs();
    db.set_setting(STARTED_KEY, &now.to_string())
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 同步状态写入失败"))?;
    let session =
        match ima_client::refresh(http, &ima_client::base(), &cfg.uid, &cfg.refresh_token).await {
            Ok(session) => session,
            Err(err) => {
                record_result(
                db,
                &json!({"status": "error", "error": format!("IMA 登录失败: {}", safe_ima(&err))}),
            )
            .await;
                return Err(CollectorError::Unavailable("IMA 登录失败"));
            }
        };
    let mut listed = 0_i64;
    let mut failed_groups = Vec::new();
    let mut skipped_folders = 0_usize;
    let mut pending = 0_i64;
    let mut retried = 0_usize;
    let mut downloaded = 0_i64;
    let mut download_errors = 0_i64;
    let mut first_download_error = String::new();
    let mut done = Vec::new();
    for group in targets {
        let files = match ima_client::list_pdfs_deep(
            http,
            &ima_client::base(),
            &session,
            &group.knowledge_base_id,
            &mount_ids(group),
        )
        .await
        {
            Ok((files, skipped)) => {
                skipped_folders += skipped;
                files
            }
            Err(err) => {
                tracing::warn!(group = group.id, "IMA 知识库列表读取失败: {err}");
                failed_groups.push(json!({"group": group.id, "error": safe_ima(&err)}));
                continue;
            }
        };
        let have = db.ima_downloaded_ids(&group.id).await.unwrap_or_default();
        let retry = db.ima_redownload_ids(&group.id).await.unwrap_or_default();
        for file in &files {
            db.record_ima_listing(&group.id, &group.name, file)
                .await
                .map_err(|_| CollectorError::Unavailable("IMA 索引写入失败"))?;
            // 以前对列到的每个文件都直接 fetch_pdf。库里 legacy 一万七千行只下过 311 个，
            // 全量补档一轮要十几小时（上游每个请求 4 秒），所以只看最近几天的新研报。
            let fresh = ima_client::fresh_sort_date(&file.sort_date, 3);
            let Some(is_retry) =
                download_wanted(&file.media_id, fresh, &have, &retry, &mut retried)
            else {
                continue;
            };
            pending += 1;
            let Some(root) = root else {
                continue;
            };
            match ima_client::fetch_pdf(
                http,
                &ima_client::base(),
                &session,
                &group.knowledge_base_id,
                &file.media_id,
            )
            .await
            {
                Err(err) => {
                    download_errors += 1;
                    if is_retry {
                        let _ = db
                            .note_ima_redownload_failure(&group.id, &file.media_id)
                            .await;
                    }
                    if first_download_error.is_empty() {
                        first_download_error = safe_ima(&err);
                    }
                }
                Ok(bytes) => match save_pdf_off(root, &group.id, file, bytes).await {
                    Err(err) => {
                        download_errors += 1;
                        if is_retry {
                            let _ = db
                                .note_ima_redownload_failure(&group.id, &file.media_id)
                                .await;
                        }
                        if first_download_error.is_empty() {
                            first_download_error = err;
                        }
                    }
                    Ok(relative) => {
                        if db
                            .mark_ima_pdf(&group.id, &file.media_id, &relative)
                            .await
                            .is_ok()
                        {
                            downloaded += 1;
                        } else {
                            download_errors += 1;
                            if first_download_error.is_empty() {
                                first_download_error = "PDF 索引写入失败".into();
                            }
                        }
                    }
                },
            }
        }
        listed += files.len() as i64;
        done.push(group.id.clone());
    }
    if done.is_empty() && !failed_groups.is_empty() {
        // Nothing was listed at all: retry before the full interval, backing
        // off so an IMA outage does not mean a login + listing every minute.
        let streak = text(db, FAIL_STREAK_KEY)
            .await
            .unwrap_or_default()
            .parse::<u32>()
            .unwrap_or(0)
            .saturating_add(1);
        let _ = db.set_setting(FAIL_STREAK_KEY, &streak.to_string()).await;
        let _ = db
            .set_setting(
                STARTED_KEY,
                &retry_started_at(now, cfg.interval_seconds, streak).to_string(),
            )
            .await;
    } else if !done.is_empty() {
        let _ = db.set_setting(FAIL_STREAK_KEY, "0").await;
    }
    let finished = now_secs();
    let result = json!({
        "status": if failed_groups.is_empty() { "ok" } else { "partial" },
        "listed": listed,
        "pending": pending,
        "groups": done,
        "failed_groups": failed_groups,
        "skipped_folders": skipped_folders,
        "downloaded": downloaded,
        "download_errors": download_errors,
        "download_error": first_download_error
    });
    db.set_setting(FINISHED_KEY, &finished.to_string())
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 同步状态写入失败"))?;
    db.set_setting(RESULT_KEY, &result.to_string())
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 同步状态写入失败"))?;
    Ok(json!({"ok": true, "result": result}))
}

/// Collision repairs re-fetched per sync round; keeps one round short even if
/// many legacy rows were cleared.
const REDOWNLOAD_PER_ROUND: usize = 20;

/// `Some(is_retry)` when the file should be fetched this round. Old files are
/// only fetched when the collision repair queued them, and at most
/// `REDOWNLOAD_PER_ROUND` of those per round.
fn download_wanted(
    media_id: &str,
    fresh: bool,
    have: &HashSet<String>,
    retry: &HashSet<String>,
    retried: &mut usize,
) -> Option<bool> {
    if have.contains(media_id) {
        return None;
    }
    if fresh {
        return Some(false);
    }
    if !retry.contains(media_id) || *retried >= REDOWNLOAD_PER_ROUND {
        return None;
    }
    *retried += 1;
    Some(true)
}

async fn save_pdf_off(
    root: &std::path::Path,
    group_id: &str,
    file: &ima_client::File,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let (root, group_id, file) = (root.to_path_buf(), group_id.to_string(), file.clone());
    tokio::task::spawn_blocking(move || save_pdf(&root, &group_id, &file, &bytes))
        .await
        .unwrap_or_else(|_| Err("PDF 写入失败".into()))
}

async fn record_result(db: &Db, result: &Value) {
    let _ = db.set_setting(FINISHED_KEY, &now_secs().to_string()).await;
    let _ = db.set_setting(RESULT_KEY, &result.to_string()).await;
}

struct Cfg {
    uid: String,
    refresh_token: String,
    knowledge_base_id: String,
    root_folder_id: String,
    interval_seconds: i64,
    groups: Vec<Group>,
}

async fn load(db: &Db) -> Result<Cfg, String> {
    let kb = setting_or(db, KB_KEY, "IMA_KB_ID", "7464369361259867").await?;
    let root = setting_or(db, ROOT_KEY, "IMA_ROOT_FOLDER_ID", &kb).await?;
    let interval =
        global_interval(&setting_or(db, INTERVAL_KEY, "IMA_INTERVAL_SECONDS", "3600").await?);
    let groups = match db
        .setting(GROUPS_KEY)
        .await
        .map_err(|err| err.to_string())?
    {
        None => vec![legacy(&kb, &root)],
        Some(raw) => parse_groups(&raw, &kb, &root),
    };
    Ok(Cfg {
        uid: setting_or(db, UID_KEY, "IMA_UID", "001aa361168019ef").await?,
        refresh_token: setting_or(db, TOKEN_KEY, "IMA_REFRESH_TOKEN", "").await?,
        knowledge_base_id: kb,
        root_folder_id: root,
        interval_seconds: interval,
        groups,
    })
}

fn config_public(cfg: &Cfg) -> Value {
    let configured = !cfg.uid.is_empty()
        && !cfg.refresh_token.is_empty()
        && cfg.groups.iter().any(|group| {
            group.enabled && !group.knowledge_base_id.is_empty() && !mount_ids(group).is_empty()
        });
    json!({
        "uid": cfg.uid,
        "knowledge_base_id": cfg.knowledge_base_id,
        "root_folder_id": cfg.root_folder_id,
        "interval_seconds": cfg.interval_seconds,
        "refresh_token": {"set": !cfg.refresh_token.is_empty(), "preview": if cfg.refresh_token.is_empty() { "" } else { "已保存" }},
        "configured": configured
    })
}

fn group_public(group: &Group) -> Value {
    let folder_ids = mount_ids(group);
    json!({
        "id": group.id,
        "name": group.name,
        "knowledge_base_id": group.knowledge_base_id,
        "root_folder_id": group.root_folder_id,
        "folder_ids": folder_ids,
        "mounted_folder_count": folder_ids.len(),
        "enabled": group.enabled && !folder_ids.is_empty(),
        "source": group.source,
        "interval_seconds": group.interval_seconds
    })
}

fn mount_ids(group: &Group) -> Vec<String> {
    match &group.folder_ids {
        None if group.enabled && !group.root_folder_id.is_empty() => {
            vec![group.root_folder_id.clone()]
        }
        None => Vec::new(),
        Some(ids) => ids.clone(),
    }
}

fn normalize_groups(incoming: &[GroupIn], existing: &[Group]) -> Result<Value, CollectorError> {
    let mut rows = Vec::new();
    let mut seen = Vec::new();
    for group in incoming {
        let name = group.name.trim();
        let kb = group.knowledge_base_id.trim();
        let root = group.root_folder_id.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(CollectorError::Bad("IMA 群组名称不能为空且最多 100 个字符"));
        }
        if !ident(kb, 64, false) || !ident(root, 128, false) {
            return Err(CollectorError::Bad("知识库 ID 格式无效"));
        }
        let id = match group.id.as_deref().map(str::trim) {
            None | Some("") => format!("manual-{}", &sha_id(&format!("{kb}\0{root}"))[..16]),
            Some(id) => {
                if !ident(id, 128, true) {
                    return Err(CollectorError::Bad("IMA 群组 ID 格式无效"));
                }
                if id.starts_with("local-") {
                    return Err(CollectorError::Bad(
                        "local- 前缀专供本地库，IMA 群组不得使用",
                    ));
                }
                id.to_string()
            }
        };
        if seen.iter().any(|seen: &String| seen == &id) {
            return Err(CollectorError::Bad("IMA 群组 ID 不能重复"));
        }
        seen.push(id.clone());
        let previous = existing.iter().find(|item| item.id == id);
        let folder_ids = match &group.folder_ids {
            None => previous.map(mount_ids).unwrap_or_else(|| {
                if group.enabled.unwrap_or(true) {
                    vec![root.to_string()]
                } else {
                    Vec::new()
                }
            }),
            Some(raw) => {
                if raw.len() > 256 {
                    return Err(CollectorError::Bad("每个 IMA 群组最多挂载 256 个文件夹"));
                }
                let mut ids = Vec::new();
                for item in raw {
                    let Some(folder) = item.as_str().map(str::trim) else {
                        return Err(CollectorError::Bad("文件夹 ID 格式无效"));
                    };
                    if !ident(folder, 128, true) {
                        return Err(CollectorError::Bad("文件夹 ID 格式无效"));
                    }
                    if !ids.iter().any(|seen: &String| seen == folder) {
                        ids.push(folder.to_string());
                    }
                }
                ids
            }
        };
        let enabled = group.enabled.unwrap_or(true) && !folder_ids.is_empty();
        let source = previous
            .map(|item| item.source.clone())
            .unwrap_or_else(|| "manual".into());
        let interval = group_interval(
            group
                .interval_seconds
                .or(previous.map(|item| item.interval_seconds)),
        );
        rows.push(json!({
            "id": id,
            "name": name,
            "knowledge_base_id": kb,
            "root_folder_id": root,
            "folder_ids": if enabled { folder_ids } else { Vec::<String>::new() },
            "enabled": enabled,
            "source": if source == "discovered" { "discovered" } else { "manual" },
            "interval_seconds": interval
        }));
    }
    Ok(Value::Array(rows))
}

fn parse_groups(raw: &str, kb: &str, root: &str) -> Vec<Group> {
    let Ok(Value::Array(rows)) = serde_json::from_str(raw) else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    for item in rows {
        let Some(id) = item["id"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            return Vec::new();
        };
        if id.starts_with("local-") {
            continue;
        }
        let id = if id == "legacy" { "legacy" } else { id };
        let kb = if id == "legacy" {
            kb
        } else {
            item["knowledge_base_id"].as_str().unwrap_or("").trim()
        };
        let root = if id == "legacy" {
            root
        } else {
            item["root_folder_id"].as_str().unwrap_or("").trim()
        };
        if kb.is_empty() || root.is_empty() {
            return Vec::new();
        }
        let enabled = match item["enabled"] {
            Value::Bool(value) => value,
            _ => return Vec::new(),
        };
        let folder_ids = if item.get("folder_ids").is_none() || item["folder_ids"].is_null() {
            None
        } else {
            let Some(list) = item["folder_ids"].as_array() else {
                return Vec::new();
            };
            if list.len() > 256 {
                return Vec::new();
            }
            let mut ids = Vec::new();
            for folder in list {
                let Some(folder) = folder.as_str().map(str::trim) else {
                    return Vec::new();
                };
                if !ident(folder, 128, true) {
                    return Vec::new();
                }
                ids.push(folder.to_string());
            }
            Some(if enabled { ids } else { Vec::new() })
        };
        groups.push(Group {
            id: id.to_string(),
            name: item["name"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .take(100)
                .collect(),
            knowledge_base_id: kb.to_string(),
            root_folder_id: root.to_string(),
            enabled,
            source: if item["source"] == "discovered" {
                "discovered".into()
            } else {
                "manual".into()
            },
            folder_ids,
            interval_seconds: group_interval(item["interval_seconds"].as_i64()),
        });
    }
    groups
}

fn legacy(kb: &str, root: &str) -> Group {
    Group {
        id: "legacy".into(),
        name: "IMA 文档".into(),
        knowledge_base_id: kb.into(),
        root_folder_id: root.into(),
        enabled: true,
        source: "manual".into(),
        folder_ids: None,
        interval_seconds: 3600,
    }
}

/// 落盘路径 `ima/{group}/{YYYY}/{MMDD}/{media_id}.pdf`：按库、按年隔开，文件名只用 media_id，
/// 同名研报不会互相覆盖，也不会因中文标题过长超过文件名上限。标题只存在 DB 里。
fn save_pdf(
    root: &std::path::Path,
    group_id: &str,
    file: &ima_client::File,
    bytes: &[u8],
) -> Result<String, String> {
    if !bytes.starts_with(b"%PDF") {
        return Err("IMA 返回的不是 PDF".into());
    }
    std::fs::create_dir_all(root).map_err(|_| "归档目录不可用".to_string())?;
    let day = if file.day.len() == 4 && file.day.bytes().all(|b| b.is_ascii_digit()) {
        file.day.clone()
    } else {
        "unknown".into()
    };
    let year = file
        .sort_date
        .get(..4)
        .filter(|year| year.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("0000");
    let relative = format!(
        "ima/{}/{year}/{day}/{}.pdf",
        path_component(group_id),
        path_component(&file.media_id)
    );
    let path = root.join(&relative);
    let dir = path.parent().ok_or_else(|| "归档目录不可用".to_string())?;
    std::fs::create_dir_all(dir).map_err(|_| "归档目录不可用".to_string())?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "归档目录不可用".to_string())?;
    let canonical_dir = dir
        .canonicalize()
        .map_err(|_| "归档目录不可用".to_string())?;
    if !canonical_dir.starts_with(&canonical_root) {
        return Err("PDF 路径越界".into());
    }
    write_atomic(&path, bytes).map_err(|_| "PDF 写入失败".to_string())?;
    Ok(relative)
}

/// 先写同目录下的临时文件并 fsync，再 rename 覆盖，崩溃时不会留下截断的 PDF。
pub(crate) fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = path.with_file_name(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let result = (|| {
        let mut out = std::fs::File::create(&tmp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 目录/文件名片段只保留 `[A-Za-z0-9_-]`；不满足或过长时用 SHA-256 前 32 位代替。
fn path_component(value: &str) -> String {
    if (1..=96).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        value.to_string()
    } else {
        sha_id(value)[..32].to_string()
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Backdated start time that makes `too_soon` allow the next run after
/// 60s, 120s, 240s, ... consecutive all-failed rounds, never later than a
/// normal interval.
fn retry_started_at(now: i64, interval: i64, streak: u32) -> i64 {
    let backoff = 60_i64
        .saturating_mul(1_i64 << streak.saturating_sub(1).min(20))
        .min(interval);
    now - interval + backoff
}

async fn too_soon(cfg: &Cfg, db: &Db) -> Result<Option<i64>, String> {
    let last = text(db, STARTED_KEY).await?.parse::<i64>().unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if last > 0 && now - last < cfg.interval_seconds {
        return Ok(Some(last + cfg.interval_seconds));
    }
    Ok(None)
}

async fn discovery(db: &Db) -> Result<Value, String> {
    let value = json_setting(db, DISCOVERY_KEY).await?;
    Ok(json!({
        "status": value["status"].as_str().unwrap_or("idle"),
        "at": value["at"].as_str().unwrap_or(""),
        "error": value["error"].as_str().unwrap_or("")
    }))
}

async fn json_setting(db: &Db, key: &str) -> Result<Value, String> {
    let raw = text(db, key).await?;
    Ok(serde_json::from_str(&raw).unwrap_or(Value::Null))
}

async fn text(db: &Db, key: &str) -> Result<String, String> {
    Ok(db
        .setting(key)
        .await
        .map_err(|err| err.to_string())?
        .unwrap_or_default())
}

async fn setting_or(db: &Db, key: &str, env: &str, default: &str) -> Result<String, String> {
    let saved = text(db, key).await?;
    if !saved.trim().is_empty() {
        return Ok(saved.trim().to_string());
    }
    Ok(std::env::var(env)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string()))
}

fn global_interval(value: &str) -> i64 {
    value
        .parse::<i64>()
        .map(|number| number.clamp(1800, 604_800))
        .unwrap_or(3600)
}

fn group_interval(value: Option<i64>) -> i64 {
    match value {
        Some(number) if number >= 43200 => 86400,
        Some(number) if number >= 10800 => 21600,
        _ => 3600,
    }
}

fn ident(value: &str, max: usize, colon: bool) -> bool {
    (1..=max).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || (colon && byte == b':')
        })
}

fn sha_id(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FolderFake;

    impl ima_client::Transport for FolderFake {
        async fn post(&self, url: &str, _: &[(&str, String)], _: &str) -> Result<Value, String> {
            if url.contains("refresh") {
                return Ok(json!({"token": "abc"}));
            }
            Ok(
                json!({"knowledge_list": [{"folder_info": {"folder_id": "folder_a", "name": "八月"}}], "next_cursor": ""}),
            )
        }
    }

    struct Fake;

    impl ima_client::Transport for Fake {
        async fn post(&self, url: &str, _: &[(&str, String)], _: &str) -> Result<Value, String> {
            if url.contains("refresh") {
                return Ok(json!({"token": "abc"}));
            }
            Ok(
                json!({"is_end": true, "knowledge_list": [{"media_id": "m1", "title": "报告.pdf", "file_size": 12, "create_time": 1700000000000i64}]}),
            )
        }
    }

    async fn db() -> Db {
        let path = std::env::temp_dir().join(format!(
            "vpush-ima-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Db::open(&path).await.unwrap()
    }

    #[tokio::test]
    async fn saves_mount_and_refuses_to_pretend_sync_started() {
        let db = db().await;
        let saved = save(
            &db,
            &SaveBody {
                uid: Some("uid1".into()),
                refresh_token: Some("token".into()),
                knowledge_base_id: None,
                root_folder_id: None,
                interval_seconds: Some(3600),
                groups: Some(vec![GroupIn {
                    id: Some("kb1".into()),
                    name: "库".into(),
                    knowledge_base_id: "kb1".into(),
                    root_folder_id: "root".into(),
                    enabled: Some(true),
                    folder_ids: Some(vec![json!("folder")]),
                    interval_seconds: Some(100),
                }]),
            },
        )
        .await
        .unwrap();
        assert_eq!(saved["config"]["refresh_token"]["preview"], "已保存");
        assert_eq!(saved["config"]["configured"], true);
        assert_eq!(saved["config"]["groups"][0]["interval_seconds"], 3600);
        let err = sync(
            &db,
            &SyncBody {
                group_id: Some("missing".into()),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CollectorError::NotFound(_)));
        assert!(matches!(
            folders(&db, "kb1", "bad id").await.unwrap_err(),
            CollectorError::Bad(_)
        ));
        let cfg = load(&db).await.unwrap();
        let folders = list_group_folders(&cfg, &cfg.groups[0], "root", &FolderFake)
            .await
            .unwrap();
        assert_eq!(folders["items"][0]["id"], "folder_a");
        let listed = sync_groups(&db, &cfg, &cfg.groups, &Fake, None)
            .await
            .unwrap();
        assert_eq!(listed["result"]["listed"], 1);
        assert_eq!(listed["result"]["downloaded"], 0);
        assert_eq!(db.ima_document_count().await.unwrap(), 1);
        let root = std::env::temp_dir().join(format!("vpush-pdf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = ima_client::File {
            media_id: "m1".into(),
            name: "报告.pdf".into(),
            day: "1114".into(),
            sort_date: "2023-11-14".into(),
            size: 12,
            text: String::new(),
        };
        let relative = save_pdf(&root, "kb1", &file, b"%PDF-1.4\n").unwrap();
        assert_eq!(relative, "ima/kb1/2023/1114/m1.pdf");
        let other = save_pdf(&root, "kb2", &file, b"%PDF-1.5\n").unwrap();
        assert_eq!(other, "ima/kb2/2023/1114/m1.pdf");
        assert_eq!(std::fs::read(root.join(&relative)).unwrap(), b"%PDF-1.4\n");
        assert_eq!(std::fs::read(root.join(&other)).unwrap(), b"%PDF-1.5\n");
        let long = ima_client::File {
            media_id: "媒体/../x".repeat(40),
            name: "很长的标题".repeat(60),
            ..file.clone()
        };
        let odd = save_pdf(&root, "kb1", &long, b"%PDF-1.4\n").unwrap();
        assert!(odd.starts_with("ima/kb1/2023/1114/"));
        assert!(!odd.contains(".."));
        assert!(std::fs::read_dir(root.join("ima/kb1/2023/1114"))
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")));
        db.mark_ima_pdf("kb1", "m1", &relative).await.unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(save(
            &db,
            &SaveBody {
                uid: Some("bad uid".into()),
                refresh_token: None,
                knowledge_base_id: None,
                root_folder_id: None,
                interval_seconds: None,
                groups: None,
            }
        )
        .await
        .is_err());
    }

    #[test]
    fn grantable_group_id_matches_the_admin_route() {
        assert!(grantable_group_id("legacy"));
        assert!(grantable_group_id("local-cicc-research"));
        assert!(grantable_group_id("7479082602225992"));
        assert!(!grantable_group_id("bad id"));
        assert!(!grantable_group_id(&"x".repeat(129)));
    }

    struct GrantEnv {
        previous: Option<std::ffi::OsString>,
    }

    impl GrantEnv {
        fn set(value: &str) -> Self {
            let previous = std::env::var_os("WEB_GRANT_DEFAULT_IMA_KB");
            std::env::set_var("WEB_GRANT_DEFAULT_IMA_KB", value);
            Self { previous }
        }
    }

    impl Drop for GrantEnv {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("WEB_GRANT_DEFAULT_IMA_KB", value),
                None => std::env::remove_var("WEB_GRANT_DEFAULT_IMA_KB"),
            }
        }
    }

    async fn seed_research_libraries(db: &Db) {
        let groups = json!([
            {"id": "legacy", "name": "旧库", "knowledge_base_id": "7464369361259867", "root_folder_id": "7464369361259867", "folder_ids": ["7464369361259867"], "enabled": true, "source": "manual", "interval_seconds": 3600},
            {"id": "7479082602225992", "name": "库A", "knowledge_base_id": "7479082602225992", "root_folder_id": "root-a", "folder_ids": ["root-a"], "enabled": true, "source": "manual", "interval_seconds": 3600},
            {"id": "new-library-9001", "name": "新库", "knowledge_base_id": "9001", "root_folder_id": "root-new", "folder_ids": ["root-new"], "enabled": true, "source": "manual", "interval_seconds": 3600},
            {"id": "disabled-kb", "name": "关闭", "knowledge_base_id": "111", "root_folder_id": "root-off", "folder_ids": ["root-off"], "enabled": false, "source": "manual", "interval_seconds": 3600},
            {"id": "unmounted", "name": "未挂载", "knowledge_base_id": "222", "root_folder_id": "root-empty", "folder_ids": [], "enabled": true, "source": "manual", "interval_seconds": 3600},
            {"id": "feishu-open", "name": "飞书", "knowledge_base_id": "333", "root_folder_id": "root-f", "folder_ids": ["root-f"], "enabled": true, "source": "manual", "interval_seconds": 3600}
        ]);
        db.set_setting(GROUPS_KEY, &groups.to_string())
            .await
            .unwrap();
        let local = json!({
            "scanned_at": "2026-10-14T00:00:00Z",
            "libraries": [
                {"slug": "cicc-research", "group_id": "local-cicc-research", "name": "中金研究", "enabled": true, "error": ""},
                {"slug": "draft", "group_id": "local-draft", "name": "草稿", "enabled": false, "error": ""},
                {"slug": "broken", "group_id": "local-broken", "name": "损坏", "enabled": true, "error": "标记文件不可读"}
            ]
        });
        db.set_setting("ima_local_libraries", &local.to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn new_user_gets_enabled_libraries_admin_is_left_alone_and_failures_are_logged() {
        let _lock = lock_grant_env().await;
        let _on = GrantEnv::set("1");
        let db = db().await;
        seed_research_libraries(&db).await;
        let configured = [
            "legacy",
            "7479082602225992",
            "new-library-9001",
            "local-cicc-research",
        ];
        assert_eq!(
            enabled_research_group_ids(&db).await.unwrap(),
            configured
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>()
        );

        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        sqlx::query("INSERT INTO ima_kb_acl (group_id, user_id) VALUES ('kept', ?)")
            .bind(admin.id)
            .execute(db.pool())
            .await
            .unwrap();
        grant_default_libraries(&db, admin.id).await;
        assert_eq!(
            db.ima_kb_group_ids_for_user(admin.id).await.unwrap(),
            vec!["kept".to_string()]
        );
        assert!(db
            .ima_kb_subscribed_group_ids_for_user(admin.id)
            .await
            .unwrap()
            .is_empty());

        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin) VALUES ('readerkb', 'x', 0)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let reader = db.user_by_username("readerkb").await.unwrap().unwrap();
        grant_default_libraries(&db, reader.id).await;
        let mut stored: Vec<String> = configured.iter().map(|id| (*id).to_string()).collect();
        stored.sort();
        assert_eq!(
            db.ima_kb_group_ids_for_user(reader.id).await.unwrap(),
            stored
        );
        assert_eq!(
            db.ima_kb_subscribed_group_ids_for_user(reader.id)
                .await
                .unwrap(),
            stored
        );
        assert_eq!(
            db.ima_kb_group_ids_for_user(admin.id).await.unwrap(),
            vec!["kept".to_string()]
        );
        let listed = db.list_admin_users().await.unwrap();
        let row = listed
            .iter()
            .find(|row| row["username"] == "readerkb")
            .unwrap();
        assert_eq!(row["ima_kb_groups"], json!(stored));

        {
            let _off = GrantEnv::set("0");
            assert!(!default_library_grant_enabled());
            sqlx::query(
                "INSERT INTO users (username, password_hash, is_admin) VALUES ('readeroff', 'x', 0)",
            )
            .execute(db.pool())
            .await
            .unwrap();
            let off = db.user_by_username("readeroff").await.unwrap().unwrap();
            grant_default_libraries(&db, off.id).await;
            assert!(db
                .ima_kb_group_ids_for_user(off.id)
                .await
                .unwrap()
                .is_empty());
        }
        for value in ["false", "False"] {
            let _off = GrantEnv::set(value);
            assert!(!default_library_grant_enabled(), "{value}");
        }
        assert!(default_library_grant_enabled());

        sqlx::query("DROP TABLE ima_kb_acl")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin) VALUES ('readerfail', 'x', 0)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let failed = db.user_by_username("readerfail").await.unwrap().unwrap();
        grant_default_libraries(&db, failed.id).await;
        assert!(db.user_by_id(failed.id).await.unwrap().is_some());
    }

    struct HalfBroken;

    impl ima_client::Transport for HalfBroken {
        async fn post(&self, url: &str, _: &[(&str, String)], body: &str) -> Result<Value, String> {
            if url.contains("refresh") {
                return Ok(json!({"token": "abc"}));
            }
            if body.contains("kb_bad") {
                return Err("timeout".into());
            }
            Ok(
                json!({"is_end": true, "knowledge_list": [{"media_id": "ok1", "title": "报告.pdf", "file_size": 12, "create_time": 1700000000000i64}]}),
            )
        }
    }

    fn group_in(id: &str) -> GroupIn {
        GroupIn {
            id: Some(id.into()),
            name: id.into(),
            knowledge_base_id: id.into(),
            root_folder_id: "root".into(),
            enabled: Some(true),
            folder_ids: Some(vec![json!("folder")]),
            interval_seconds: Some(100),
        }
    }

    #[tokio::test]
    async fn one_failing_group_does_not_stop_the_others_and_runs_are_exclusive() {
        let db = db().await;
        save(
            &db,
            &SaveBody {
                uid: Some("uid1".into()),
                refresh_token: Some("token".into()),
                knowledge_base_id: None,
                root_folder_id: None,
                interval_seconds: Some(3600),
                groups: Some(vec![group_in("kb_bad"), group_in("kb_good")]),
            },
        )
        .await
        .unwrap();
        let cfg = load(&db).await.unwrap();
        let out = sync_groups(&db, &cfg, &cfg.groups, &HalfBroken, None)
            .await
            .unwrap();
        assert_eq!(out["result"]["status"], "partial");
        assert_eq!(out["result"]["groups"], json!(["kb_good"]));
        assert_eq!(out["result"]["failed_groups"][0]["group"], "kb_bad");
        assert_eq!(out["result"]["listed"], 1);

        let bad: Vec<Group> = cfg
            .groups
            .iter()
            .filter(|group| group.id == "kb_bad")
            .cloned()
            .collect();
        for streak in 1..=2_i64 {
            sync_groups(&db, &cfg, &bad, &HalfBroken, None)
                .await
                .unwrap();
            assert_eq!(
                text(&db, FAIL_STREAK_KEY).await.unwrap(),
                streak.to_string()
            );
            let next = too_soon(&cfg, &db).await.unwrap().expect("backing off");
            let wait = next - now_secs();
            assert!((60 * streak - 2..=60 * streak).contains(&wait), "{wait}");
        }
        sync_groups(&db, &cfg, &cfg.groups, &HalfBroken, None)
            .await
            .unwrap();
        assert_eq!(text(&db, FAIL_STREAK_KEY).await.unwrap(), "0");

        let guard = RunGuard::acquire().expect("free");
        assert!(RunGuard::acquire().is_none());
        assert_eq!(status(&db).await.unwrap()["running"], true);
        let queued = start_sync(
            &db,
            &SyncBody {
                group_id: Some("kb_good".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(queued["status"], "already_running");
        assert!(matches!(
            sync(&db, &SyncBody { group_id: None }).await,
            Err(CollectorError::TooSoon)
        ));
        drop(guard);
        assert_eq!(status(&db).await.unwrap()["running"], false);
    }

    #[test]
    fn only_queued_old_files_are_redownloaded_and_capped_per_round() {
        let have = HashSet::from(["done".to_string()]);
        let retry: HashSet<String> = (0..30).map(|i| format!("r{i}")).collect();
        let mut retried = 0;
        assert_eq!(
            download_wanted("done", true, &have, &retry, &mut retried),
            None
        );
        assert_eq!(
            download_wanted("new", true, &have, &retry, &mut retried),
            Some(false)
        );
        assert_eq!(
            download_wanted("legacy", false, &have, &retry, &mut retried),
            None
        );
        let picked = (0..30)
            .filter(|i| {
                download_wanted(&format!("r{i}"), false, &have, &retry, &mut retried).is_some()
            })
            .count();
        assert_eq!(picked, REDOWNLOAD_PER_ROUND);
        assert_eq!(
            download_wanted("new2", true, &have, &retry, &mut retried),
            Some(false)
        );
    }

    #[test]
    fn all_failed_rounds_back_off_exponentially_up_to_the_interval() {
        let now = 1_000_000;
        let next_run = |streak| retry_started_at(now, 3600, streak) + 3600 - now;
        assert_eq!(next_run(1), 60);
        assert_eq!(next_run(2), 120);
        assert_eq!(next_run(3), 240);
        assert_eq!(next_run(6), 1920);
        assert_eq!(next_run(7), 3600);
        assert_eq!(next_run(u32::MAX), 3600);
    }
}
