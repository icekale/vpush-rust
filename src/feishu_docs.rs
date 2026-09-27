//! 已落盘的飞书文档时间线。不采集、不写文件。
//! `feishu-` 开头的库登录即可读。其它库要同时有授权和订阅。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::db::Db;

struct Gate {
    admin: bool,
    acl: HashSet<String>,
    subscribed: HashSet<String>,
}

impl Gate {
    fn readable(&self, gid: &str) -> bool {
        self.admin
            || gid.starts_with("feishu-")
            || (self.acl.contains(gid) && self.subscribed.contains(gid))
    }
}

async fn gate(db: &Db, user_id: i64, is_admin: bool) -> Result<Gate, &'static str> {
    if is_admin {
        return Ok(Gate {
            admin: true,
            acl: HashSet::new(),
            subscribed: HashSet::new(),
        });
    }
    let (acl, subscribed) = db.ima_access(user_id).await.map_err(|_| "读取文档失败")?;
    Ok(Gate {
        admin: false,
        acl,
        subscribed,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn timeline_all(
    db: &Db,
    root: &Path,
    user_id: i64,
    is_admin: bool,
    group: &str,
    order: &str,
    window_days: Option<i64>,
    before: &str,
) -> Result<Value, &'static str> {
    if !before.is_empty() && window_days.is_none() {
        return Err("时间线游标需要窗口参数");
    }
    let latest = order != "original";
    let gate = gate(db, user_id, is_admin).await?;
    let sources = db
        .active_feishu_sources()
        .await
        .map_err(|_| "读取文档失败")?;
    let readable: Vec<Value> = sources
        .into_iter()
        .filter(|source| gate.readable(source["group_id"].as_str().unwrap_or("")))
        .collect();
    if !group.is_empty() && !readable.iter().any(|source| source["group_id"] == group) {
        return Err("文档不存在");
    }
    let mut public_sources = Vec::new();
    let mut entries = Vec::new();
    let mut notices = Vec::new();
    for source in &readable {
        let gid = source["group_id"].as_str().unwrap_or("");
        let public = json!({
            "id": source["id"],
            "group_id": gid,
            "media_id": source["media_id"],
            "title": source["title"].as_str().filter(|s| !s.is_empty()).unwrap_or("飞书文档"),
            "canonical_url": source["canonical_url"],
            "revision_id": source["revision_id"],
            "last_success_at": source["last_success_at"],
        });
        public_sources.push(public.clone());
        if !group.is_empty() && gid != group {
            continue;
        }
        let Some(data) = read_timeline(root, source["timeline_path"].as_str().unwrap_or("")) else {
            continue;
        };
        for item in data["notices"].as_array().into_iter().flatten() {
            notices.push(with_source(item, &public));
        }
        for item in data["entries"].as_array().into_iter().flatten() {
            entries.push(with_source(item, &public));
        }
    }
    let (page, has_more, next_cursor) = page_entries(&entries, latest, window_days, before)?;
    Ok(json!({
        "sources": public_sources,
        "notices": notices,
        "entries": page,
        "order": if latest { "latest" } else { "original" },
        "has_more": has_more,
        "next_cursor": next_cursor,
    }))
}

fn with_source(entry: &Value, source: &Value) -> Value {
    let mut map = entry.as_object().cloned().unwrap_or_default();
    map.insert("source".into(), source.clone());
    Value::Object(map)
}

fn push_catalog_group(
    groups: &mut Vec<Value>,
    gid: &str,
    title: &str,
    day: &str,
    media: &str,
    add: i64,
) {
    if let Some(group) = groups.iter_mut().find(|group| group["id"] == gid) {
        group["document_count"] = json!(group["document_count"].as_i64().unwrap_or(0) + add);
        if day > group["latest_day"].as_str().unwrap_or("") {
            group["latest_day"] = json!(day);
            group["latest_title"] = json!(title);
            group["latest_media_id"] = json!(media);
        }
        return;
    }
    groups.push(json!({
        "id": gid,
        "name": title,
        "enabled": true,
        "document_count": add,
        "latest_day": day,
        "latest_title": title,
        "latest_media_id": media,
    }));
}

fn archive_ok(root: &Path, relative: &str) -> bool {
    root.canonicalize()
        .ok()
        .and_then(|root| safe_path(&root, relative))
        .is_some_and(|path| path.is_file())
}

pub async fn catalog(db: &Db, user_id: i64, is_admin: bool) -> Result<Value, &'static str> {
    let gate = gate(db, user_id, is_admin).await?;
    let sources = db
        .active_feishu_sources()
        .await
        .map_err(|_| "读取文档失败")?;
    let index = db.ima_index().await.map_err(|_| "读取文档失败")?;
    let mut seen = HashSet::new();
    let mut groups: Vec<Value> = Vec::new();
    for source in sources {
        let gid = source["group_id"].as_str().unwrap_or("").to_string();
        let media = source["media_id"].as_str().unwrap_or("").to_string();
        if gid.is_empty() || !(gate.readable(&gid) || gate.acl.contains(&gid)) {
            continue;
        }
        seen.insert(format!("{gid}\0{media}"));
        let title = source["title"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or(&gid)
            .to_string();
        let day = source["last_success_at"].as_str().unwrap_or("");
        let day = if day.len() >= 10 { &day[..10] } else { "" };
        push_catalog_group(&mut groups, &gid, &title, day, &media, 1);
    }
    for doc in &index {
        let gid = doc["group_id"].as_str().unwrap_or("");
        let media = doc["media_id"].as_str().unwrap_or("");
        if gid.is_empty()
            || !(gate.readable(gid) || gate.acl.contains(gid))
            || !seen.insert(format!("{gid}\0{media}"))
        {
            continue;
        }
        let title = doc["name"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or(gid);
        let day = doc["sort_date"].as_str().unwrap_or("");
        push_catalog_group(&mut groups, gid, title, day, media, 1);
    }
    if is_admin {
        for group in &mut groups {
            let gid = group["id"].as_str().unwrap_or("");
            let names = db
                .ima_kb_acl_usernames(gid)
                .await
                .map_err(|_| "读取文档失败")?;
            group["acl_usernames"] = json!(names);
        }
    }
    let (subscribed, available): (Vec<_>, Vec<_>) = groups.into_iter().partition(|group| {
        let gid = group["id"].as_str().unwrap_or("");
        gate.readable(gid)
    });
    Ok(json!({"subscribed": subscribed, "available": available}))
}

#[allow(clippy::too_many_arguments)]
pub async fn list_documents(
    db: &Db,
    user_id: i64,
    is_admin: bool,
    group: &str,
    query: &str,
    day: &str,
    tag: &str,
    limit: i64,
    offset: i64,
    facets_only: bool,
) -> Result<Value, &'static str> {
    let gate = gate(db, user_id, is_admin).await?;
    let sources = db
        .active_feishu_sources()
        .await
        .map_err(|_| "读取文档失败")?;
    let index = db.ima_index().await.map_err(|_| "读取文档失败")?;
    let visible: Vec<Value> = sources
        .into_iter()
        .filter(|source| {
            let gid = source["group_id"].as_str().unwrap_or("");
            !gid.is_empty() && gate.readable(gid)
        })
        .collect();
    let indexed: Vec<&Value> = index
        .iter()
        .filter(|doc| gate.readable(doc["group_id"].as_str().unwrap_or("")))
        .collect();
    if !group.is_empty()
        && !visible.iter().any(|source| source["group_id"] == group)
        && !indexed.iter().any(|doc| doc["group_id"] == group)
    {
        return Err("知识库不存在");
    }
    let needle = query.to_lowercase();
    let mut items: Vec<Value> = visible
        .iter()
        .filter(|source| group.is_empty() || source["group_id"] == group)
        .filter(|_| tag.is_empty())
        .filter(|source| {
            needle.is_empty()
                || source["title"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&needle)
        })
        .map(|source| {
            let raw_day = source["last_success_at"].as_str().unwrap_or("");
            let item_day = if raw_day.len() >= 10 {
                &raw_day[..10]
            } else {
                ""
            };
            let gid = source["group_id"].as_str().unwrap_or("");
            let name = source["title"]
                .as_str()
                .filter(|text| !text.is_empty())
                .unwrap_or(gid);
            json!({
                "media_id": source["media_id"],
                "name": name,
                "group_id": gid,
                "group_name": name,
                "day": item_day,
                "sort_date": item_day,
                "abstract": "",
                "downloaded_at": source["last_success_at"],
            })
        })
        .filter(|item| day.is_empty() || item["day"] == day)
        .collect();
    for doc in indexed {
        if !group.is_empty() && doc["group_id"] != group {
            continue;
        }
        if !tag.is_empty() {
            continue;
        }
        let name = doc["name"].as_str().unwrap_or("");
        if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
            continue;
        }
        let item_day = doc["sort_date"].as_str().unwrap_or("");
        if !day.is_empty() && item_day != day {
            continue;
        }
        let gid = doc["group_id"].as_str().unwrap_or("");
        let media = doc["media_id"].as_str().unwrap_or("");
        if items
            .iter()
            .any(|item| item["group_id"] == gid && item["media_id"] == media)
        {
            continue;
        }
        items.push(json!({
            "media_id": media,
            "name": name,
            "group_id": gid,
            "group_name": doc["group_name"],
            "day": item_day,
            "sort_date": item_day,
            "abstract": doc["abstract"],
            "downloaded_at": doc["downloaded_at"],
            "has_pdf": !doc["pdf_path"].as_str().unwrap_or("").is_empty(),
            "has_txt": !doc["txt_path"].as_str().unwrap_or("").is_empty(),
        }));
    }
    items.sort_by(|a, b| {
        b["sort_date"]
            .as_str()
            .unwrap_or("")
            .cmp(a["sort_date"].as_str().unwrap_or(""))
    });
    let total = items.len();
    let mut days: Vec<&str> = items
        .iter()
        .filter_map(|item| item["day"].as_str())
        .filter(|day| !day.is_empty())
        .collect();
    days.sort_unstable();
    days.dedup();
    days.reverse();
    let start = offset.max(0) as usize;
    let limit = limit.clamp(1, 50) as usize;
    let page = if facets_only || start >= items.len() {
        Vec::new()
    } else {
        items[start..items.len().min(start + limit)].to_vec()
    };
    Ok(json!({
        "groups": [],
        "items": page,
        "days": days,
        "tags": [],
        "tag_counts": {},
        "document_count": total,
        "day": day,
        "has_more": !facets_only && start + page.len() < total,
        "offset": if facets_only { 0 } else { start },
    }))
}

pub async fn document_meta(
    db: &Db,
    user_id: i64,
    is_admin: bool,
    media_id: &str,
    group: &str,
    root: &Path,
) -> Result<Value, &'static str> {
    let access = gate(db, user_id, is_admin).await?;
    let indexed = db
        .ima_documents_for_media(media_id, group)
        .await
        .map_err(|_| "读取文档失败")?;
    let indexed: Vec<Value> = indexed
        .into_iter()
        .filter(|doc| access.readable(doc["group_id"].as_str().unwrap_or("")))
        .collect();
    if let Some(doc) = match (group.is_empty(), indexed.len()) {
        (false, 1..) => indexed.into_iter().next(),
        (true, 1) => indexed.into_iter().next(),
        _ => None,
    } {
        let gid = doc["group_id"].as_str().unwrap_or("").to_string();
        let feishu = db
            .feishu_document(media_id, &gid)
            .await
            .map_err(|_| "读取文档失败")?;
        let name = doc["name"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or("文档");
        let abstract_text = doc["abstract"].as_str().unwrap_or("");
        let (abstract_zh, needs_translation) =
            translation_fields(db, &gid, media_id, abstract_text).await?;
        return Ok(json!({
            "media_id": doc["media_id"],
            "name": name,
            "day": doc["day"],
            "size": doc["size"],
            "chars": doc["chars"],
            "downloaded_at": doc["downloaded_at"],
            "group_id": gid,
            "group_name": doc["group_name"],
            "abstract": abstract_text,
            "abstract_zh": abstract_zh,
            "needs_translation": needs_translation,
            "cover_url": "",
            "tags": [],
            "has_pdf": archive_ok(root, doc["pdf_path"].as_str().unwrap_or("")),
            "has_txt": archive_ok(root, doc["txt_path"].as_str().unwrap_or("")),
            "type": if feishu.is_some() { "feishu_timeline" } else { "document" },
            "source_url": feishu.as_ref().and_then(|row| row["canonical_url"].as_str()).unwrap_or(""),
            "feishu_display": if feishu.is_some() { "timeline" } else { "" },
        }));
    }
    let Some(row) = db
        .feishu_document(media_id, group)
        .await
        .map_err(|_| "读取文档失败")?
    else {
        return Err("文档不存在");
    };
    let gid = row["group_id"].as_str().unwrap_or("");
    if !access.readable(gid) {
        return Err("文档不存在");
    }
    let title = row["title"]
        .as_str()
        .filter(|text| !text.is_empty())
        .unwrap_or("飞书文档");
    Ok(json!({
        "media_id": row["media_id"],
        "name": title,
        "day": "",
        "size": 0,
        "chars": 0,
        "downloaded_at": row["last_success_at"],
        "group_id": gid,
        "group_name": "",
        "abstract": "",
        "abstract_zh": "",
        "needs_translation": false,
        "cover_url": "",
        "tags": [],
        "has_pdf": false,
        "has_txt": false,
        "type": if gid.starts_with("feishu-") { "feishu_timeline" } else { "document" },
        "source_url": row["canonical_url"],
        "feishu_display": "timeline",
    }))
}

pub async fn translate_document<F>(
    db: &Db,
    user_id: i64,
    is_admin: bool,
    media_id: &str,
    group: &str,
    root: &Path,
    translate: F,
) -> Result<Value, &'static str>
where
    F: FnOnce(&str) -> String,
{
    let meta = document_meta(db, user_id, is_admin, media_id, group, root).await?;
    let source = meta["abstract"].as_str().unwrap_or("");
    if meta["needs_translation"] != true {
        let cached = meta["abstract_zh"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or(source);
        return Ok(json!({"abstract_zh": cached}));
    }
    let translated = translate(source);
    let chosen = if collapsed(&translated, source) {
        source.to_string()
    } else {
        translated
    };
    if chosen != source {
        let gid = meta["group_id"].as_str().unwrap_or("");
        db.save_ima_abstract_translation(gid, media_id, &src_hash(source), &chosen)
            .await
            .map_err(|_| "读取文档失败")?;
    }
    Ok(json!({"abstract_zh": chosen}))
}

pub fn my_memory(text: &str) -> String {
    let query: String = text.chars().take(500).collect();
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(8))
        .build();
    let response = agent
        .get("https://api.mymemory.translated.net/get")
        .query("q", &query)
        .query("langpair", "en|zh-CN")
        .call();
    let Ok(response) = response else {
        return text.to_string();
    };
    let body = response.into_string().unwrap_or_default();
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    parsed["responseData"]["translatedText"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(text)
        .to_string()
}

async fn translation_fields(
    db: &Db,
    group_id: &str,
    media_id: &str,
    text: &str,
) -> Result<(String, bool), &'static str> {
    if text.is_empty() {
        return Ok((String::new(), false));
    }
    let (hash, cached) = db
        .ima_abstract_translation(group_id, media_id)
        .await
        .map_err(|_| "读取文档失败")?;
    let fresh = !cached.is_empty() && hash == src_hash(text);
    let needs = !already_chinese(text) && !fresh;
    Ok((if fresh { cached } else { String::new() }, needs))
}

fn already_chinese(text: &str) -> bool {
    let cjk = text
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    if cjk < 4 {
        return false;
    }
    let foreign = strip_links(text)
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic() || ('\u{3040}'..='\u{30ff}').contains(ch))
        .count();
    cjk * 4 >= foreign * 3
}

fn strip_links(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("http://").or_else(|| rest.find("https://")) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn collapsed(translated: &str, source: &str) -> bool {
    let text = translated.trim();
    let original = source.trim();
    text.is_empty()
        || text == original
        || (text.chars().count() <= 4 && original.chars().count() >= 20)
}

fn src_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub async fn archive_file(
    db: &Db,
    root: &Path,
    user_id: i64,
    is_admin: bool,
    media_id: &str,
    group: &str,
    kind: &str,
) -> Result<(PathBuf, String), &'static str> {
    let access = gate(db, user_id, is_admin).await?;
    let indexed = db
        .ima_documents_for_media(media_id, group)
        .await
        .map_err(|_| "读取文档失败")?;
    let indexed: Vec<Value> = indexed
        .into_iter()
        .filter(|doc| access.readable(doc["group_id"].as_str().unwrap_or("")))
        .collect();
    let doc = match (group.is_empty(), indexed.len()) {
        (false, 1..) => indexed.into_iter().next(),
        (true, 1) => indexed.into_iter().next(),
        _ => None,
    };
    let Some(doc) = doc else {
        return Err("文档不存在");
    };
    let missing = if kind == "pdf" {
        "PDF 文件不存在"
    } else {
        "TXT 文件不存在"
    };
    let relative = doc[if kind == "pdf" {
        "pdf_path"
    } else {
        "txt_path"
    }]
    .as_str()
    .unwrap_or("");
    let path = root
        .canonicalize()
        .ok()
        .and_then(|root| safe_path(&root, relative))
        .filter(|path| path.is_file())
        .ok_or(missing)?;
    let name = doc["name"]
        .as_str()
        .filter(|text| !text.is_empty())
        .unwrap_or(if kind == "pdf" {
            "document.pdf"
        } else {
            "document.txt"
        });
    Ok((path, name.to_string()))
}

pub async fn asset_file(
    db: &Db,
    root: &Path,
    user_id: i64,
    is_admin: bool,
    media_id: &str,
    group: &str,
    asset_id: &str,
) -> Result<PathBuf, &'static str> {
    if asset_id.len() != 64 || !asset_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("飞书文档资源不存在");
    }
    let Some((group_id, asset_root)) = db
        .feishu_asset_root(media_id, group)
        .await
        .map_err(|_| "读取文档失败")?
    else {
        return Err("飞书文档不存在");
    };
    if !gate(db, user_id, is_admin).await?.readable(&group_id) {
        return Err("飞书文档不存在");
    }
    find_asset(root, &asset_root, asset_id).ok_or("飞书文档资源不存在")
}

fn find_asset(root: &Path, asset_root: &str, asset_id: &str) -> Option<PathBuf> {
    let archive = root.canonicalize().ok()?;
    let dir = safe_path(&archive, asset_root)?;
    if !dir.is_dir() {
        return None;
    }
    let prefix = format!("{asset_id}.");
    let mut found = None;
    for entry in std::fs::read_dir(&dir).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(&prefix) || !entry.file_type().ok()?.is_file() {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(entry.path());
    }
    let path = found?.canonicalize().ok()?;
    path.starts_with(&archive).then_some(path)
}

pub fn asset_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

fn read_timeline(root: &Path, relative: &str) -> Option<Value> {
    let root = root.canonicalize().ok()?;
    let path = safe_path(&root, relative)?;
    let data: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    data.is_object().then_some(data)
}

fn safe_path(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains('\0') || Path::new(relative).is_absolute() {
        return None;
    }
    let candidate = root.join(relative);
    let path = candidate.canonicalize().ok()?;
    path.starts_with(root).then_some(path)
}

fn page_entries(
    entries: &[Value],
    latest: bool,
    window_days: Option<i64>,
    before: &str,
) -> Result<(Vec<Value>, bool, String), &'static str> {
    let mut ordered = entries.to_vec();
    ordered.sort_by_key(|b| std::cmp::Reverse(entry_key(b)));
    if !latest {
        ordered.reverse();
    }
    let Some(window) = window_days else {
        return Ok((ordered, false, String::new()));
    };
    let window = window.clamp(1, 31);
    let cursor = if before.is_empty() {
        None
    } else {
        Some(cursor_key(before)?)
    };
    let candidates: Vec<Value> = ordered
        .into_iter()
        .filter(|item| match &cursor {
            None => true,
            Some(key) if latest => entry_key(item) < *key,
            Some(key) => entry_key(item) > *key,
        })
        .collect();
    let Some(anchor_day) = candidates.first().and_then(entry_day) else {
        return Ok((Vec::new(), false, String::new()));
    };
    let anchor = day_ord(&anchor_day).ok_or("时间线游标无效")?;
    let page: Vec<Value> = candidates
        .iter()
        .filter(|item| {
            let Some(day) = entry_day(item).and_then(|day| day_ord(&day)) else {
                return false;
            };
            if latest {
                day <= anchor && day >= anchor - (window - 1)
            } else {
                day >= anchor && day <= anchor + (window - 1)
            }
        })
        .cloned()
        .collect();
    if page.is_empty() {
        return Ok((Vec::new(), !candidates.is_empty(), String::new()));
    }
    let has_more = page.len() < candidates.len();
    let next = if has_more {
        cursor_of(page.last().unwrap())
    } else {
        String::new()
    };
    Ok((page, has_more, next))
}

fn entry_key(entry: &Value) -> (String, String) {
    (
        entry["timestamp"].as_str().unwrap_or("").to_string(),
        entry["id"].as_str().unwrap_or("").to_string(),
    )
}

fn entry_day(entry: &Value) -> Option<String> {
    let day = entry["day"]
        .as_str()
        .filter(|s| s.len() >= 10)
        .map(|s| s[..10].to_string())
        .or_else(|| {
            entry["timestamp"]
                .as_str()
                .filter(|s| s.len() >= 10)
                .map(|s| s[..10].to_string())
        })?;
    day_ord(&day).map(|_| day)
}

fn cursor_of(entry: &Value) -> String {
    let (timestamp, id) = entry_key(entry);
    let raw = json!({"timestamp": timestamp, "id": id}).to_string();
    base64_url(raw.as_bytes())
}

fn cursor_key(cursor: &str) -> Result<(String, String), &'static str> {
    let bytes = base64_url_decode(cursor).map_err(|_| "时间线游标无效")?;
    let payload: Value = serde_json::from_slice(&bytes).map_err(|_| "时间线游标无效")?;
    let timestamp = payload["timestamp"].as_str().unwrap_or("").to_string();
    let id = payload["id"].as_str().unwrap_or("").to_string();
    if timestamp.is_empty() || id.is_empty() {
        return Err("时间线游标无效");
    }
    Ok((timestamp, id))
}

fn base64_url(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(T[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(T[(n & 63) as usize] as char);
        }
    }
    out
}

fn base64_url_decode(raw: &str) -> Result<Vec<u8>, ()> {
    let mut padded = raw.replace('-', "+").replace('_', "/");
    while !padded.len().is_multiple_of(4) {
        padded.push('=');
    }
    let mut out = Vec::new();
    let bytes = padded.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let vals: Vec<u8> = bytes[i..i + 4]
            .iter()
            .map(|c| b64_val(*c))
            .collect::<Result<_, _>>()?;
        let n = ((vals[0] as u32) << 18)
            | ((vals[1] as u32) << 12)
            | ((vals[2] as u32) << 6)
            | vals[3] as u32;
        out.push((n >> 16) as u8);
        if bytes[i + 2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if bytes[i + 3] != b'=' {
            out.push(n as u8);
        }
        i += 4;
    }
    Ok(out)
}

fn b64_val(c: u8) -> Result<u8, ()> {
    match c {
        b'A'..=b'Z' => Ok(c - b'A'),
        b'a'..=b'z' => Ok(c - b'a' + 26),
        b'0'..=b'9' => Ok(c - b'0' + 52),
        b'+' | b'-' => Ok(62),
        b'/' | b'_' => Ok(63),
        b'=' => Ok(0),
        _ => Err(()),
    }
}

fn day_ord(day: &str) -> Option<i64> {
    let mut parts = day.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = (year - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    Some(era as i64 * 146097 + doe as i64 - 719468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pages_open_timelines_and_hides_private_groups() {
        let dir = std::env::temp_dir().join(format!("vpush-feishu-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("vpush-feishu-secret-{}.json", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(&outside, br#"{"entries":[{"id":"x","timestamp":"2024-01-10T12:00:00","day":"2024-01-10","text":"secret"}]}"#).unwrap();
        std::fs::write(
            dir.join("docs/timeline.json"),
            br#"{"entries":[
                {"id":"a","timestamp":"2024-01-10T10:00:00","day":"2024-01-10","text":"new"},
                {"id":"b","timestamp":"2024-01-09T10:00:00","day":"2024-01-09","text":"mid"},
                {"id":"c","timestamp":"2024-01-01T10:00:00","day":"2024-01-01","text":"old"}
            ],"notices":[]}"#,
        )
        .unwrap();
        let db = Db::open(&dir.join("t.db")).await.unwrap();
        db.insert_feishu_source("feishu-open", "m1", "公开", "docs/timeline.json", "")
            .await
            .unwrap();
        db.insert_feishu_source("private-kb", "m2", "私有", "docs/timeline.json", "")
            .await
            .unwrap();
        let escape = format!("../{}", outside.file_name().unwrap().to_string_lossy());
        db.insert_feishu_source("feishu-bad", "m3", "越界", &escape, "")
            .await
            .unwrap();

        let page = timeline_all(&db, &dir, 0, false, "", "latest", Some(2), "")
            .await
            .unwrap();
        let titles: Vec<_> = page["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, vec!["公开", "越界"]);
        assert_eq!(page["entries"].as_array().unwrap().len(), 2);
        assert_eq!(page["entries"][0]["text"], "new");
        assert!(page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["text"] != "secret"));
        assert_eq!(page["has_more"], true);
        let older = timeline_all(
            &db,
            &dir,
            0,
            false,
            "",
            "latest",
            Some(2),
            page["next_cursor"].as_str().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(older["entries"][0]["text"], "old");
        assert!(
            timeline_all(&db, &dir, 0, false, "private-kb", "latest", Some(7), "")
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
    }

    #[tokio::test]
    async fn serves_one_archive_image_and_rejects_the_rest() {
        let dir = std::env::temp_dir().join(format!("vpush-feishu-asset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let asset = "a".repeat(64);
        std::fs::create_dir_all(dir.join("docs/assets")).unwrap();
        std::fs::write(dir.join(format!("docs/assets/{asset}.png")), b"png").unwrap();
        let db = Db::open(&dir.join("t.db")).await.unwrap();
        db.insert_feishu_source(
            "feishu-open",
            "m1",
            "公开",
            "docs/timeline.json",
            "docs/assets",
        )
        .await
        .unwrap();
        db.insert_feishu_source(
            "private-kb",
            "m2",
            "私有",
            "docs/timeline.json",
            "docs/assets",
        )
        .await
        .unwrap();
        let path = asset_file(&db, &dir, 0, false, "m1", "feishu-open", &asset)
            .await
            .unwrap();
        assert_eq!(asset_type(&path), "image/png");
        assert!(asset_file(&db, &dir, 0, false, "m2", "private-kb", &asset)
            .await
            .is_err());
        assert!(
            asset_file(&db, &dir, 0, false, "m1", "feishu-open", "../secret")
                .await
                .is_err()
        );
        let meta = document_meta(&db, 0, false, "m1", "feishu-open", &dir)
            .await
            .unwrap();
        assert_eq!(meta["name"], "公开");
        assert_eq!(meta["type"], "feishu_timeline");
        let listed = catalog(&db, 0, false).await.unwrap();
        assert_eq!(listed["subscribed"][0]["id"], "feishu-open");
        assert_eq!(listed["available"].as_array().unwrap().len(), 0);
        let admin = catalog(&db, 0, true).await.unwrap();
        assert!(admin["subscribed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["id"] == "private-kb"));
        assert!(listed["subscribed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|group| group["id"] != "private-kb"));
        let docs = list_documents(&db, 0, false, "", "公开", "", "", 50, 0, false)
            .await
            .unwrap();
        assert_eq!(docs["items"][0]["name"], "公开");
        assert_eq!(docs["document_count"], 1);
        assert!(
            list_documents(&db, 0, false, "private-kb", "", "", "", 50, 0, false)
                .await
                .is_err()
        );
        assert_eq!(meta["has_pdf"], false);
        assert!(document_meta(&db, 0, false, "m2", "private-kb", &dir)
            .await
            .is_err());
        std::fs::write(dir.join(format!("docs/assets/{asset}.jpg")), b"jpg").unwrap();
        assert!(asset_file(&db, &dir, 0, false, "m1", "feishu-open", &asset)
            .await
            .is_err());
        db.set_ima_kb_acl("private-kb", &[7]).await.unwrap();
        assert!(
            timeline_all(&db, &dir, 7, false, "private-kb", "latest", Some(7), "")
                .await
                .is_ok()
        );
        db.ima_kb_unsubscribe(7, "private-kb").await.unwrap();
        let after = catalog(&db, 7, false).await.unwrap();
        assert!(after["available"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["id"] == "private-kb"));
        assert!(after["subscribed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|group| group["id"] != "private-kb"));
        assert!(document_meta(&db, 7, false, "m2", "private-kb", &dir)
            .await
            .is_err());
        db.ima_kb_subscribe(7, "private-kb").await.unwrap();
        assert_eq!(
            document_meta(&db, 7, false, "m2", "private-kb", &dir)
                .await
                .unwrap()["name"],
            "私有"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn serves_archive_pdf_and_text_to_subscribers_only() {
        let dir = std::env::temp_dir().join(format!("vpush-ima-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("reports")).unwrap();
        std::fs::write(dir.join("reports/note.pdf"), b"%PDF-1.4").unwrap();
        std::fs::write(dir.join("reports/note.txt"), "正文").unwrap();
        let outside =
            std::env::temp_dir().join(format!("vpush-ima-secret-{}.pdf", std::process::id()));
        std::fs::write(&outside, b"%PDF-secret").unwrap();
        let db = Db::open(&dir.join("t.db")).await.unwrap();
        db.insert_ima_document(
            "reports",
            "pdf1",
            "研报",
            "2024-01-02",
            "reports/note.pdf",
            "reports/note.txt",
        )
        .await
        .unwrap();
        let escape = format!("../{}", outside.file_name().unwrap().to_string_lossy());
        db.insert_ima_document(
            "reports",
            "pdf2",
            "越界",
            "2024-01-01",
            &escape,
            "reports/note.txt",
        )
        .await
        .unwrap();

        assert!(archive_file(&db, &dir, 7, false, "pdf1", "reports", "pdf")
            .await
            .is_err());
        db.set_ima_kb_acl("reports", &[7]).await.unwrap();
        db.ima_kb_unsubscribe(7, "reports").await.unwrap();
        let hidden = catalog(&db, 7, false).await.unwrap();
        assert!(hidden["available"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["id"] == "reports"));
        assert!(archive_file(&db, &dir, 7, false, "pdf1", "reports", "pdf")
            .await
            .is_err());
        db.ima_kb_subscribe(7, "reports").await.unwrap();

        let (path, name) = archive_file(&db, &dir, 7, false, "pdf1", "reports", "pdf")
            .await
            .unwrap();
        assert_eq!(name, "研报");
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.4");
        let (text_path, _) = archive_file(&db, &dir, 7, false, "pdf1", "reports", "txt")
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(text_path).unwrap(), "正文");
        assert!(archive_file(&db, &dir, 7, false, "pdf2", "reports", "pdf")
            .await
            .is_err());
        let meta = document_meta(&db, 7, false, "pdf1", "reports", &dir)
            .await
            .unwrap();
        assert_eq!(meta["has_pdf"], true);
        assert_eq!(meta["has_txt"], true);
        assert_eq!(meta["type"], "document");
        let listed = list_documents(&db, 7, false, "reports", "研报", "", "", 50, 0, false)
            .await
            .unwrap();
        assert_eq!(listed["items"][0]["name"], "研报");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
    }

    #[tokio::test]
    async fn translates_an_english_abstract_once() {
        let path = std::env::temp_dir().join(format!(
            "vpush-tr-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.insert_ima_document("reports", "en1", "Note", "2024-01-02", "", "")
            .await
            .unwrap();
        sqlx::query("UPDATE ima_document_index SET abstract = ? WHERE media_id = 'en1'")
            .bind("Revenue grew and the company raised its full-year guidance.")
            .execute(db.pool())
            .await
            .unwrap();
        db.set_ima_kb_acl("reports", &[7]).await.unwrap();
        db.ima_kb_subscribe(7, "reports").await.unwrap();
        let root = PathBuf::new();
        let meta = document_meta(&db, 7, false, "en1", "reports", &root)
            .await
            .unwrap();
        assert_eq!(meta["needs_translation"], true);
        let translated = translate_document(&db, 7, false, "en1", "reports", &root, |_| {
            "收入增长，公司上调了全年指引。".into()
        })
        .await
        .unwrap();
        assert_eq!(translated["abstract_zh"], "收入增长，公司上调了全年指引。");
        let again = document_meta(&db, 7, false, "en1", "reports", &root)
            .await
            .unwrap();
        assert_eq!(again["needs_translation"], false);
        let cached =
            translate_document(&db, 7, false, "en1", "reports", &root, |_| panic!("cached"))
                .await
                .unwrap();
        assert_eq!(cached["abstract_zh"], "收入增长，公司上调了全年指引。");
        let kept = translate_document(&db, 7, false, "missing", "reports", &root, |_| {
            "不会用到".into()
        });
        assert!(kept.await.is_err());
        let _ = std::fs::remove_file(&path);
    }
}
