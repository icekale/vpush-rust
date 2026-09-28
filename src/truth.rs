//! Truth Social。官网接口有防护，这里读 CNN 公开存档的头部。
//! 存档只有川普的帖，不会写到其他 Truth 账号上。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::db::Db;

const ARCHIVE: &str = "https://ix.cnn.io/data/truth-social/truth_archive.json";
const TRUMP: &str = "realdonaldtrump";

struct Entry {
    external_id: String,
    id_num: i64,
    title: String,
    content: String,
    url: String,
    images: Vec<String>,
    published_at: String,
    published_unix: Option<i64>,
}

pub fn spawn(db: Db) {
    if !crate::xueqiu::fetch_enabled() {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Err(err) = poll(&db).await {
                tracing::warn!("Truth 抓取: {err}");
                let _ = db.note_platform("truth", Some(&err)).await;
            } else {
                let _ = db.note_platform("truth", None).await;
            }
            tokio::time::sleep(Duration::from_secs(crate::xueqiu::poll_wait(&db).await)).await;
        }
    });
}

async fn poll(db: &Db) -> Result<(), String> {
    let kols = db.kols_to_fetch("truth").await.map_err(|e| e.to_string())?;
    let Some((id, name, _)) = kols
        .into_iter()
        .find(|(_, _, external_id)| is_trump(external_id))
    else {
        return Ok(());
    };
    let exit = crate::proxy_admin::acquire(db, "truth").await?;
    let proxy = exit.as_ref().map(|item| item.url.clone());
    let proxy_id = exit.map(|item| item.id);
    let fetched = tokio::task::spawn_blocking(move || fetch_head(proxy.as_deref()))
        .await
        .map_err(|e| e.to_string())?;
    let raw = match fetched {
        Ok(raw) => {
            crate::proxy_admin::note(db, proxy_id, true, "").await;
            raw
        }
        Err(err) => {
            crate::proxy_admin::note(db, proxy_id, false, &err).await;
            return Err(err);
        }
    };
    let entries = parse_archive_head(&raw)?;
    let last = db.max_external_num(id).await.map_err(|e| e.to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for entry in entries {
        if entry.id_num <= last {
            continue;
        }
        let Some(unix) = entry.published_unix else {
            continue;
        };
        let age = now.saturating_sub(unix);
        if age > 36 * 3600 {
            continue;
        }
        if db
            .has_post("truth", &entry.external_id)
            .await
            .map_err(|e| e.to_string())?
        {
            continue;
        }
        let images = serde_json::to_string(&entry.images).unwrap_or_else(|_| "[]".into());
        db.save_fetched(
            id,
            &entry.external_id,
            &entry.title,
            &entry.content,
            "post",
            &images,
            &entry.url,
            &entry.published_at,
        )
        .await
        .map_err(|e| e.to_string())?;
        if age > 60 * 60
            || !db
                .should_push(id, "post")
                .await
                .map_err(|e| e.to_string())?
        {
            continue;
        }
        crate::push::deliver(
            db,
            id,
            &crate::feishu::Note {
                kol_name: &name,
                platform: "truth",
                external_id: &entry.external_id,
                post_type: "post",
                title: &entry.title,
                content: &entry.content,
                url: &entry.url,
                published_at: &entry.published_at,
            },
        )
        .await;
    }
    Ok(())
}

fn is_trump(external_id: &str) -> bool {
    let id = external_id
        .trim()
        .trim_start_matches('@')
        .to_ascii_lowercase();
    id.is_empty() || id == TRUMP
}

fn fetch_head(proxy: Option<&str>) -> Result<String, String> {
    let agent =
        crate::proxy_admin::http_agent(proxy, Duration::from_secs(15), Duration::from_secs(30))?;
    let response = agent
        .get(ARCHIVE)
        .set("Accept-Encoding", "identity")
        .set("Range", "bytes=0-524287")
        .call();
    match response {
        Ok(resp) if resp.status() == 200 || resp.status() == 206 => {
            resp.into_string().map_err(|e| e.to_string())
        }
        Ok(resp) => Err(format!("Truth 存档 HTTP {}", resp.status())),
        Err(ureq::Error::Status(code, resp)) if code == 200 || code == 206 => {
            resp.into_string().map_err(|e| e.to_string())
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("Truth 存档 HTTP {code}")),
        Err(err) => Err(err.to_string()),
    }
}

fn parse_archive_head(raw: &str) -> Result<Vec<Entry>, String> {
    let cut = raw.rfind("},\n").unwrap_or(0);
    if cut == 0 {
        return Err("存档窗口内没有完整条目".into());
    }
    let text = format!("{}]", &raw[..=cut]);
    let rows: Vec<Value> =
        serde_json::from_str(&text).map_err(|_| "存档头部不是完整 JSON".to_string())?;
    Ok(rows.into_iter().filter_map(entry_from).collect())
}

fn entry_from(row: Value) -> Option<Entry> {
    let external_id = match &row["id"] {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let id_num = external_id.parse::<i64>().ok()?;
    let content = strip(&field(&row, "content"));
    let images = images(&row);
    if content.is_empty() && images.is_empty() {
        return None;
    }
    let url = field(&row, "url");
    let url = if url.starts_with("https://") {
        url
    } else {
        format!("https://truthsocial.com/@realDonaldTrump/{external_id}")
    };
    let (published_at, published_unix) = published(&field(&row, "created_at"));
    let title = content
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(80)
        .collect::<String>();
    let title = if title.is_empty() {
        "图片".into()
    } else {
        title
    };
    Some(Entry {
        external_id,
        id_num,
        title,
        content: if content.is_empty() {
            "图片".into()
        } else {
            content
        },
        url,
        images,
        published_at,
        published_unix,
    })
}

fn images(row: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(media) = row.get("media").and_then(Value::as_array) else {
        return out;
    };
    for item in media {
        let url = item.as_str().unwrap_or("").trim();
        let path = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
        if url.starts_with("https://")
            && [".jpg", ".jpeg", ".png", ".webp", ".gif", ".mp4"]
                .iter()
                .any(|ext| path.ends_with(ext))
            && out.len() < 4
        {
            out.push(url.to_string());
        }
    }
    out
}

fn published(raw: &str) -> (String, Option<i64>) {
    let raw = raw.trim();
    let parsed = (|| {
        if raw.len() < 19 {
            return None;
        }
        let year: i32 = raw[0..4].parse().ok()?;
        let month: u32 = raw[5..7].parse().ok()?;
        let day: u32 = raw[8..10].parse().ok()?;
        let hour: i64 = raw[11..13].parse().ok()?;
        let minute: i64 = raw[14..16].parse().ok()?;
        let second: i64 = raw[17..19].parse().ok()?;
        let local = days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second;
        let unix = local - offset_secs(raw);
        let beijing = unix + 8 * 3600;
        let days = beijing.div_euclid(86400);
        let sod = beijing.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        let text = format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02}",
            sod / 3600,
            (sod % 3600) / 60
        );
        Some((text, unix))
    })();
    match parsed {
        Some((text, unix)) => (text, Some(unix)),
        None => (raw.to_string(), None),
    }
}

fn offset_secs(raw: &str) -> i64 {
    if raw.ends_with('Z') {
        return 0;
    }
    let bytes = raw.as_bytes();
    let Some(pos) = bytes.iter().rposition(|b| *b == b'+' || *b == b'-') else {
        return 0;
    };
    if pos < 19 {
        return 0;
    }
    let sign = if bytes[pos] == b'-' { -1 } else { 1 };
    let rest = &raw[pos + 1..];
    let hour: i64 = rest.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let minute: i64 = rest
        .get(3..5)
        .or_else(|| rest.get(2..4))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    sign * (hour * 3600 + minute * 60)
}

fn strip(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('>')
            .map(|n| start + n)
            .unwrap_or(rest.len() - 1);
        if rest[start..=end].to_ascii_lowercase().starts_with("</p") {
            out.push('\n');
        }
        rest = if end + 1 < rest.len() {
            &rest[end + 1..]
        } else {
            ""
        };
    }
    out.push_str(rest);
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

fn field(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
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

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

pub async fn backfill<F, Fut>(db: &Db, limit: i64, mut translate: F) -> Result<i64, sqlx::Error>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT id, title, content FROM posts
         WHERE platform = 'truth' AND content_src = '' AND length(content) >= 20
         AND published_at >= datetime('now', '-1 day')
         ORDER BY id DESC LIMIT ?",
    )
    .bind(limit.saturating_mul(3).max(1))
    .fetch_all(db.pool())
    .await?;
    let mut done = 0;
    for row in rows {
        if done >= limit {
            break;
        }
        let id: i64 = row.get("id");
        let title: String = row.get("title");
        let content: String = row.get("content");
        if !backfill_candidate(&content) {
            store_translation(db, id, &title, &content, &title, &content).await?;
            continue;
        }
        let translated = match translate(content.clone()).await {
            Ok(text) => text,
            Err(_) => continue,
        };
        let translated = translated.trim().to_string();
        if translated.is_empty() || collapsed(&translated, &content) || translated == content.trim()
        {
            store_translation(db, id, &title, &content, &title, &content).await?;
            continue;
        }
        let title_zh: String = translated
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        store_translation(db, id, &title_zh, &translated, &title, &content).await?;
        done += 1;
    }
    Ok(done)
}

async fn store_translation(
    db: &Db,
    id: i64,
    title: &str,
    content: &str,
    title_src: &str,
    content_src: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE posts SET title = ?, content = ?, title_src = ?, content_src = ? WHERE id = ?",
    )
    .bind(title)
    .bind(content)
    .bind(title_src)
    .bind(content_src)
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(())
}

fn backfill_candidate(text: &str) -> bool {
    let value = text.trim();
    if value.chars().count() < 20 {
        return false;
    }
    let bare = strip_links(value).trim().to_string();
    if bare.chars().count() < 8 || already_chinese(value) {
        return false;
    }
    value.chars().filter(|ch| ch.is_ascii_alphabetic()).count() >= 8
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
        .filter(|ch| ch.is_ascii_alphabetic())
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
    text.is_empty() || (text.chars().count() <= 4 && original.chars().count() >= 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_head_keeps_complete_posts() {
        let raw = "[\n  {\"id\": \"20\", \"created_at\": \"2026-09-26T04:30:00Z\", \"content\": \"<p>你好</p><p>世界</p>\", \"media\": [\"https://cdn.example/a.jpg\", \"https://cdn.example/skip.txt\"], \"url\": \"https://truthsocial.com/@realDonaldTrump/20\"},\n  {\"id\": \"21\", \"created_at\": \"broken\"";
        let entries = parse_archive_head(raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "你好\n世界");
        assert_eq!(entries[0].published_at, "2026-09-26 12:30");
        assert_eq!(
            entries[0].images,
            vec!["https://cdn.example/a.jpg".to_string()]
        );
        assert!(is_trump("realDonaldTrump"));
        assert!(!is_trump("someone"));
    }

    #[tokio::test]
    async fn backfill_translates_recent_english_and_marks_the_rest() {
        let path = std::env::temp_dir().join(format!(
            "vpush-truth-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query(
            "INSERT INTO posts (platform, kol_id, external_id, title, content, published_at) VALUES
             ('truth', 1, 'a', 'Title a', 'This is a long enough English post number one for the backfill test.', datetime('now')),
             ('truth', 1, 'b', 'Title b', 'This is a long enough English post number two for the backfill test.', datetime('now')),
             ('truth', 1, 'c', 'Title c', 'This is a long enough English post number three for the backfill test.', datetime('now')),
             ('truth', 1, 'link', 'RT', 'RT: https://truthsocial.com/users/realDonaldTrump/statuses/111111', datetime('now')),
             ('truth', 1, 'zh', '中文', '这是一条足够长的中文帖子内容，本来就不需要再做任何翻译处理了。', datetime('now')),
             ('truth', 1, 'old', 'Old', 'This old English post is two days old and outside the one day window.', '2020-01-01 00:00')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let mut calls = 0;
        let done = backfill(&db, 2, |text| {
            calls += 1;
            async move {
                Ok(format!(
                    "中文：{}",
                    text.chars().take(10).collect::<String>()
                ))
            }
        })
        .await
        .unwrap();
        assert_eq!(done, 2);
        assert_eq!(calls, 2);
        let translated: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE content LIKE '中文：%'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(translated, 2);
        let marked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM posts WHERE external_id IN ('link', 'zh') AND content_src != ''",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(marked, 2);
        let old: String =
            sqlx::query_scalar("SELECT content_src FROM posts WHERE external_id = 'old'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(old.is_empty());
        let again = backfill(&db, 3, |text| async move {
            if text.contains("three") {
                Ok(text)
            } else {
                Ok("中文新译文".into())
            }
        })
        .await
        .unwrap();
        assert_eq!(again, 1);
        let same: String =
            sqlx::query_scalar("SELECT content_src FROM posts WHERE external_id = 'c'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(same.starts_with("This is"));
        let _ = std::fs::remove_file(&path);
    }
}
