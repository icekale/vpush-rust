//! IMA 采集配置。保存 UID、令牌和知识库挂载；真正的腾讯侧下载还没接上。

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
    group_id: Option<String>,
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
        "running": false,
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

pub async fn sync(db: &Db, body: &SyncBody) -> Result<Value, CollectorError> {
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
    } else if too_soon(&cfg, db)
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 文档下载尚未接入"))?
        .is_some()
    {
        return Err(CollectorError::TooSoon);
    }
    let targets: Vec<Group> = if group_id.is_empty() {
        cfg.groups
            .iter()
            .filter(|group| group.enabled && !mount_ids(group).is_empty())
            .cloned()
            .collect()
    } else {
        cfg.groups
            .iter()
            .filter(|group| group.id == group_id)
            .cloned()
            .collect()
    };
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
    let result = sync_groups(db, &cfg, &targets, &http, root.as_deref()).await;
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
    let session = ima_client::refresh(http, &ima_client::base(), &cfg.uid, &cfg.refresh_token)
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 登录失败"))?;
    let mut listed = 0_i64;
    let mut downloaded = 0_i64;
    let mut done = Vec::new();
    for group in targets {
        let files = ima_client::list_pdfs(
            http,
            &ima_client::base(),
            &session,
            &group.knowledge_base_id,
            &mount_ids(group),
        )
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 知识库列表读取失败"))?;
        for file in &files {
            db.record_ima_listing(&group.id, &group.name, file)
                .await
                .map_err(|_| CollectorError::Unavailable("IMA 索引写入失败"))?;
            if let Some(root) = root {
                if let Ok(bytes) = ima_client::fetch_pdf(
                    http,
                    &ima_client::base(),
                    &session,
                    &group.knowledge_base_id,
                    &file.media_id,
                )
                .await
                {
                    if let Ok(relative) = save_pdf(root, file, &bytes) {
                        if db
                            .mark_ima_pdf(&group.id, &file.media_id, &relative)
                            .await
                            .is_ok()
                        {
                            downloaded += 1;
                        }
                    }
                }
            }
        }
        listed += files.len() as i64;
        done.push(group.id.clone());
    }
    let finished = now_secs();
    let result =
        json!({"status": "ok", "listed": listed, "groups": done, "downloaded": downloaded});
    db.set_setting(FINISHED_KEY, &finished.to_string())
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 同步状态写入失败"))?;
    db.set_setting(RESULT_KEY, &result.to_string())
        .await
        .map_err(|_| CollectorError::Unavailable("IMA 同步状态写入失败"))?;
    Ok(json!({"ok": true, "result": result}))
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

fn save_pdf(
    root: &std::path::Path,
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
    let name = safe_pdf_name(&file.name);
    let dir = root.join(&day);
    std::fs::create_dir_all(&dir).map_err(|_| "归档目录不可用".to_string())?;
    let path = dir.join(&name);
    std::fs::write(&path, bytes).map_err(|_| "PDF 写入失败".to_string())?;
    let root = root
        .canonicalize()
        .map_err(|_| "归档目录不可用".to_string())?;
    let full = path
        .canonicalize()
        .map_err(|_| "PDF 写入失败".to_string())?;
    if !full.starts_with(&root) {
        return Err("PDF 路径越界".into());
    }
    Ok(format!("{day}/{name}"))
}

fn safe_pdf_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':') {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let out = out.trim().trim_matches('.');
    if out.is_empty() {
        "file.pdf".into()
    } else {
        out.to_string()
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
        let relative = save_pdf(&root, &file, b"%PDF-1.4\n").unwrap();
        assert_eq!(relative, "1114/报告.pdf");
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
}
