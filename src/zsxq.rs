//! 知识星球主题。使用 settings.zsxq_cookie 里的 access token，只拉第一页。
//! 抓取时只保存附件编号和文件名，文件本身等用户下载时再取。

use std::time::Duration;

use serde_json::{json, Value};

use crate::db::Db;

const TOPICS: &str = "https://api.zsxq.com/v2/groups";

struct Topic {
    external_id: String,
    title: String,
    content: String,
    url: String,
    images: Vec<String>,
    files: Vec<Value>,
    published_at: String,
    avatar: String,
}

enum Keep {
    Drop,
    Store,
    Notify,
}

pub fn spawn(db: Db) {
    if !crate::xueqiu::fetch_enabled() {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Err(err) = poll(&db).await {
                tracing::warn!("知识星球抓取: {err}");
                let _ = db.note_platform("zsxq", Some(&err)).await;
            } else {
                let _ = db.note_platform("zsxq", None).await;
            }
            tokio::time::sleep(Duration::from_secs(crate::xueqiu::poll_wait(&db).await)).await;
        }
    });
}

async fn poll(db: &Db) -> Result<(), String> {
    let kols = db.kols_to_fetch("zsxq").await.map_err(|e| e.to_string())?;
    if kols.is_empty() {
        return Ok(());
    }
    let Some(token) = token(db).await? else {
        tracing::info!("知识星球抓取跳过：未配置 zsxq_cookie");
        return Ok(());
    };
    let exit = crate::proxy_admin::acquire(db, "zsxq").await?;
    let proxy = exit.as_ref().map(|item| item.url.clone());
    let proxy_id = exit.map(|item| item.id);
    for (id, name, external_id) in kols {
        if !external_id.chars().all(|c| c.is_ascii_digit()) {
            tracing::warn!(kol = id, "知识星球外部 ID 不是数字");
            continue;
        }
        match pull(db, &token, id, &name, &external_id, proxy.clone()).await {
            Ok(()) => {
                crate::proxy_admin::note(db, proxy_id, true, "").await;
                let _ = db.note_kol_fetch(id, None).await;
            }
            Err(err) => {
                crate::proxy_admin::note(db, proxy_id, false, &err).await;
                let _ = db.note_kol_fetch(id, Some(&err)).await;
                tracing::warn!(kol = id, "{err}");
            }
        }
    }
    Ok(())
}

pub(crate) async fn access_token(db: &Db) -> Result<Option<String>, String> {
    token(db).await
}

async fn token(db: &Db) -> Result<Option<String>, String> {
    if let Some(saved) = db.setting("zsxq_cookie").await.map_err(|e| e.to_string())? {
        let saved = crate::db::zsxq_cookie_value(&saved);
        if !saved.is_empty() {
            return Ok(Some(saved));
        }
    }
    for name in ["ZSXQ_COOKIE", "ZSXQ_ACCESS_TOKEN"] {
        if let Ok(value) = std::env::var(name) {
            let value = crate::db::zsxq_cookie_value(&value);
            if !value.is_empty() {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

async fn pull(
    db: &Db,
    token: &str,
    kol_id: i64,
    name: &str,
    group_id: &str,
    proxy: Option<String>,
) -> Result<(), String> {
    let page = topics(token, group_id, proxy).await?;
    let rows = page_topics(&page)?;
    if let Some(avatar) = rows.iter().find_map(|row| {
        let avatar = topic_from(row, group_id)?.avatar;
        (!avatar.is_empty()).then_some(avatar)
    }) {
        db.set_avatar(kol_id, &avatar)
            .await
            .map_err(|e| e.to_string())?;
    }
    let watermark = db
        .max_published_at(kol_id)
        .await
        .map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for row in rows {
        let Some(topic) = topic_from(&row, group_id) else {
            continue;
        };
        let action = keep(&topic.published_at, &watermark, now);
        if matches!(action, Keep::Drop) {
            continue;
        }
        if db
            .has_post("zsxq", &topic.external_id)
            .await
            .map_err(|e| e.to_string())?
        {
            continue;
        }
        let images = serde_json::to_string(&topic.images).unwrap_or_else(|_| "[]".into());
        db.save_fetched(
            kol_id,
            &topic.external_id,
            &topic.title,
            &topic.content,
            "post",
            &images,
            &topic.url,
            &topic.published_at,
        )
        .await
        .map_err(|e| e.to_string())?;
        if !topic.files.is_empty() {
            let detail = serde_json::to_string(&json!({"files": topic.files}))
                .unwrap_or_else(|_| "{}".into());
            db.set_platform_detail("zsxq", &topic.external_id, &detail)
                .await
                .map_err(|e| e.to_string())?;
        }
        if !matches!(action, Keep::Notify)
            || !db
                .should_push(kol_id, "post")
                .await
                .map_err(|e| e.to_string())?
        {
            continue;
        }
        crate::push::deliver(
            db,
            kol_id,
            &crate::feishu::Note {
                kol_name: name,
                platform: "zsxq",
                post_type: "post",
                title: &topic.title,
                content: &topic.content,
                url: &topic.url,
                published_at: &topic.published_at,
            },
        )
        .await;
    }
    Ok(())
}

async fn topics(token: &str, group_id: &str, proxy: Option<String>) -> Result<Value, String> {
    let token = token.to_string();
    let group_id = group_id.to_string();
    let mut last = String::from("知识星球请求失败");
    for _ in 0..2 {
        let token = token.clone();
        let group_id = group_id.clone();
        let proxy = proxy.clone();
        match tokio::task::spawn_blocking(move || fetch_topics(&token, &group_id, proxy.as_deref()))
            .await
            .map_err(|e| e.to_string())?
        {
            Ok(page) => return Ok(page),
            Err(err) if err == "1059" => {
                last = "知识星球暂时拒绝，已重试".into();
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last)
}

fn fetch_topics(token: &str, group_id: &str, proxy: Option<&str>) -> Result<Value, String> {
    let url = format!("{TOPICS}/{group_id}/topics?scope=all&count=20");
    let agent =
        crate::proxy_admin::http_agent(proxy, Duration::from_secs(15), Duration::from_secs(20))?;
    let response = agent
        .get(&url)
        .set("Accept", "application/json, text/plain, */*")
        .set("Origin", "https://wx.zsxq.com")
        .set("Referer", "https://wx.zsxq.com/")
        .set(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        )
        .set("Cookie", &format!("zsxq_access_token={token}"))
        .call();
    let text = match response {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, resp)) => {
            let _ = resp.into_string();
            return Err(format!("知识星球 HTTP {code}"));
        }
        Err(err) => return Err(err.to_string()),
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|_| "知识星球响应不是 JSON".to_string())?;
    if value.get("succeeded") == Some(&json!(true)) {
        return Ok(value);
    }
    if value.get("code") == Some(&json!(1059)) {
        return Err("1059".into());
    }
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
    Err(format!("知识星球失败 code={code}"))
}

fn page_topics(page: &Value) -> Result<Vec<Value>, String> {
    Ok(page["resp_data"]["topics"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

fn topic_from(topic: &Value, group_id: &str) -> Option<Topic> {
    let external_id = match &topic["topic_id"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if external_id.is_empty() {
        return None;
    }
    let mut title = field(topic, "title");
    let mut pieces = Vec::new();
    for key in ["talk", "question", "answer", "task", "solution"] {
        let block = &topic[key];
        if !block.is_object() {
            continue;
        }
        let text = strip_e(&field(block, "text"));
        if !text.is_empty() {
            pieces.push(text);
        }
        if title.is_empty() {
            title = field(&block["article"], "title");
        }
    }
    let mut content = pieces.join("\n\n");
    let files = file_names(topic);
    if !files.is_empty() {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("附件：");
        content.push_str(&files.join("、"));
    }
    if content.is_empty() {
        content = if title.is_empty() {
            "（无声主题）".into()
        } else {
            title.clone()
        };
    }
    let avatar = field(&topic["group"]["owner"], "avatar_url");
    Some(Topic {
        external_id: external_id.clone(),
        title: if title.is_empty() {
            content.chars().take(80).collect()
        } else {
            title
        },
        content,
        url: format!("https://wx.zsxq.com/group/{group_id}/{external_id}"),
        images: images(topic),
        published_at: stamp(&field(topic, "create_time")),
        files: topic_files(topic),
        avatar,
    })
}

fn topic_files(topic: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for key in ["talk", "question", "answer", "task", "solution"] {
        let Some(files) = topic[key].get("files").and_then(Value::as_array) else {
            continue;
        };
        for file in files {
            let id = match &file["file_id"] {
                Value::String(text) => text.clone(),
                Value::Number(number) => number.to_string(),
                _ => String::new(),
            };
            if !id.chars().all(|ch| ch.is_ascii_digit()) || id.is_empty() || id.len() > 32 {
                continue;
            }
            let name = field(file, "name");
            let url = field(file, "url");
            let mut item = json!({"file_id": id, "name": name});
            if url.starts_with("https://") {
                item["url"] = json!(url);
            }
            if !out
                .iter()
                .any(|have: &Value| have["file_id"] == item["file_id"])
            {
                out.push(item);
            }
        }
    }
    out
}

fn file_names(topic: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["talk", "question", "answer", "task", "solution"] {
        let Some(files) = topic[key].get("files").and_then(Value::as_array) else {
            continue;
        };
        for file in files {
            let name = field(file, "name");
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

fn images(topic: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["talk", "question", "answer", "task", "solution"] {
        let Some(images) = topic[key].get("images").and_then(Value::as_array) else {
            continue;
        };
        for image in images {
            let chosen = if image["original"].is_object() {
                &image["original"]
            } else if image["large"].is_object() {
                &image["large"]
            } else {
                &image["thumbnail"]
            };
            let url = field(chosen, "url");
            if url.starts_with("https://") && !out.contains(&url) && out.len() < 4 {
                out.push(url);
            }
        }
    }
    out
}

fn keep(published: &str, watermark: &str, now: i64) -> Keep {
    if !watermark.is_empty() && !published.is_empty() && published < watermark {
        return Keep::Drop;
    }
    let Some(unix) = published_unix(published) else {
        return Keep::Notify;
    };
    if watermark.is_empty() && now.saturating_sub(unix) > 36 * 3600 {
        return Keep::Drop;
    }
    if now.saturating_sub(unix) > 60 * 60 {
        Keep::Store
    } else {
        Keep::Notify
    }
}

fn published_unix(published: &str) -> Option<i64> {
    if published.len() < 16 {
        return None;
    }
    let year: i32 = published[0..4].parse().ok()?;
    let month: u32 = published[5..7].parse().ok()?;
    let day: u32 = published[8..10].parse().ok()?;
    let hour: i64 = published[11..13].parse().ok()?;
    let minute: i64 = published[14..16].parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(days * 86400 + hour * 3600 + minute * 60 - 8 * 3600)
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    year -= i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = (year - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    era as i64 * 146097 + doe as i64 - 719468
}

fn stamp(raw: &str) -> String {
    let raw = raw.trim();
    if raw.len() >= 16
        && raw.as_bytes().get(4) == Some(&b'-')
        && raw
            .as_bytes()
            .get(10)
            .is_some_and(|b| *b == b'T' || *b == b' ')
    {
        return format!("{} {}", &raw[..10], &raw[11..16]);
    }
    raw.to_string()
}

fn strip_e(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('>')
            .map(|n| start + n)
            .unwrap_or(rest.len() - 1);
        let tag = &rest[start..=end];
        if tag.to_ascii_lowercase().starts_with("<e") {
            if let Some(title) = attr(tag, "title") {
                out.push_str(&percent_decode(&title));
            }
        }
        rest = if end + 1 < rest.len() {
            &rest[end + 1..]
        } else {
            ""
        };
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let at = tag.to_ascii_lowercase().find(&key)? + key.len();
    let quote = *tag.as_bytes().get(at)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value = &tag[at + 1..];
    let stop = value.find(quote as char)?;
    Some(value[..stop].to_string())
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn field(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_talk_text_image_and_time() {
        let topic = json!({
            "topic_id": 9,
            "create_time": "2026-09-26T12:30:05.000+0800",
            "talk": {
                "text": "你好 <e type=\"hashtag\" title=\"%E5%AE%8F%E8%A7%82\" />",
                "images": [{"original": {"url": "https://img.example/a.jpg"}}, {"large": {"url": "http://insecure/a.jpg"}}],
                "files": [{"name": "纪要.pdf"}]
            },
            "group": {"owner": {"avatar_url": "https://img.example/av.jpg"}}
        });
        let parsed = topic_from(&topic, "100").unwrap();
        assert_eq!(parsed.content, "你好 宏观\n\n附件：纪要.pdf");
        assert_eq!(parsed.published_at, "2026-09-26 12:30");
        assert_eq!(parsed.images, vec!["https://img.example/a.jpg".to_string()]);
        assert!(parsed.files.is_empty());
        assert_eq!(parsed.url, "https://wx.zsxq.com/group/100/9");
        assert!(matches!(
            keep("2020-01-01 00:00", "", 1_700_000_000),
            Keep::Drop
        ));
        assert!(
            page_topics(&json!({"succeeded": true, "resp_data": {"topics": []}}))
                .unwrap()
                .is_empty()
        );
    }
}
