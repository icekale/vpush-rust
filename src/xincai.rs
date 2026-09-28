//! 心裁阅读器把已抓好的正文推过来。本服务不回访文章地址。

use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::db::{CatalogError, Db, XincaiRow};
use crate::news::plain;

const MAX_BATCH: usize = 200;
const MAX_BODY: usize = 512 * 1024;

pub fn authorize(authorization: Option<&str>, expected: &str) -> Result<(), &'static str> {
    let expected = expected.trim();
    if expected.is_empty() {
        return Err("未配置 XINCAI_INGEST_TOKEN");
    }
    let token = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if token.len() != expected.len() || !bool::from(token.as_bytes().ct_eq(expected.as_bytes())) {
        return Err("令牌无效");
    }
    Ok(())
}

pub async fn ingest(db: &Db, body: &Value) -> Result<Value, CatalogError> {
    let Some(body) = body.as_object() else {
        return Err(CatalogError::Bad("请求体必须是 JSON 对象"));
    };
    let articles = body
        .get("articles")
        .and_then(|value| value.as_array())
        .ok_or(CatalogError::Bad("articles 不能为空"))?;
    if articles.is_empty() {
        return Err(CatalogError::Bad("articles 不能为空"));
    }
    if articles.len() > MAX_BATCH {
        return Err(CatalogError::Invalid(format!(
            "单批最多 {MAX_BATCH} 篇，收到 {}",
            articles.len()
        )));
    }
    let group = clip(
        body.get("group")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim(),
        40,
    );
    let default_source = clip(
        body.get("sourceName")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim(),
        60,
    );
    let hinted = body
        .get("sourceKind")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let now = now_iso();
    let mut rows = Vec::new();
    let mut skipped = 0;
    for item in articles {
        let Some(item) = item.as_object() else {
            skipped += 1;
            continue;
        };
        let Some(row) = build_row(item, &default_source, hinted, &now) else {
            skipped += 1;
            continue;
        };
        rows.push(row);
    }
    let accepted = rows.len();
    let sources = db.save_xincai(&group, &rows).await?;
    Ok(json!({"ok": true, "accepted": accepted, "skipped": skipped, "sources": sources}))
}

fn build_row(
    item: &serde_json::Map<String, Value>,
    default_source: &str,
    hinted: &str,
    now: &str,
) -> Option<XincaiRow> {
    let external_id = clip(
        item.get("externalId")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim(),
        200,
    );
    if external_id.is_empty() {
        return None;
    }
    let mut name = clip(
        item.get("sourceName")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim(),
        60,
    );
    if name.is_empty() {
        name = if default_source.is_empty() {
            "心裁".into()
        } else {
            default_source.to_string()
        };
    }
    let sid = clip(
        item.get("sourceId")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim(),
        80,
    );
    let slug = if sid.is_empty() {
        format!("xincai-name-{}", simple_hash(&name))
    } else {
        format!("xincai-{sid}")
    };
    let key = if sid.is_empty() {
        name.clone()
    } else {
        sid.clone()
    };
    let html = clip_bytes(
        item.get("html")
            .and_then(|value| value.as_str())
            .unwrap_or(""),
        MAX_BODY,
    );
    let images = image_urls(&html);
    let summary = clip(
        &plain(
            item.get("text")
                .and_then(|value| value.as_str())
                .unwrap_or(html.as_str()),
        ),
        2000,
    );
    let hint = item
        .get("sourceKind")
        .and_then(|value| value.as_str())
        .unwrap_or(hinted);
    Some(XincaiRow {
        key,
        slug,
        name: name.clone(),
        kind: publication_kind(&name, hint),
        external_id,
        title: {
            let title = clip(
                &plain(
                    item.get("title")
                        .and_then(|value| value.as_str())
                        .unwrap_or(""),
                ),
                500,
            );
            if title.is_empty() {
                "(无标题)".into()
            } else {
                title
            }
        },
        summary,
        content: html,
        url: public_url(
            item.get("url")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        author: clip(
            &plain(
                item.get("author")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
            ),
            200,
        ),
        published_at: normalize_ts(
            item.get("publishedAt")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            now,
        ),
        issue_key: issue_text(item, "issueKey", &["issueKey", "key"], 40),
        issue_label: issue_text(item, "issueLabel", &["label"], 80),
        issue_title: issue_text(item, "issueTitle", &["title"], 200),
        issue_cover: issue_text(item, "issueCover", &["cover"], 500),
        section: clip(
            item.get("section")
                .or_else(|| item.get("category"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim(),
            40,
        ),
        toc_order: item
            .get("tocOrder")
            .and_then(|value| value.as_i64())
            .unwrap_or_else(|| {
                issue_object(item)
                    .and_then(|issue| issue.get("order"))
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0)
            }),
        images,
        topics: topic_list(item),
        fetched_at: {
            let fetched = normalize_ts(
                item.get("fetchedAt")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                now,
            );
            if fetched.is_empty() {
                now.to_string()
            } else {
                fetched
            }
        },
        content_hash: clip(
            item.get("contentHash")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim(),
            64,
        ),
        platform: clip(
            item.get("platform")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim(),
            20,
        ),
    })
}

fn topic_list(item: &serde_json::Map<String, Value>) -> String {
    let Some(list) = item.get("topics").and_then(|value| value.as_array()) else {
        return "[]".into();
    };
    let names: Vec<String> = list
        .iter()
        .filter_map(|value| value.as_str())
        .map(|value| clip(value.trim(), 20))
        .filter(|value| !value.is_empty())
        .take(3)
        .collect();
    serde_json::to_string(&names).unwrap_or_else(|_| "[]".into())
}

fn issue_object(item: &serde_json::Map<String, Value>) -> Option<&serde_json::Map<String, Value>> {
    item.get("issue").and_then(|value| value.as_object())
}

fn issue_text(
    item: &serde_json::Map<String, Value>,
    flat: &str,
    nested: &[&str],
    max_chars: usize,
) -> String {
    let direct = item
        .get(flat)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if !direct.is_empty() {
        return clip(direct, max_chars);
    }
    let Some(issue) = issue_object(item) else {
        return String::new();
    };
    for key in nested {
        let text = issue
            .get(*key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if !text.is_empty() {
            return clip(text, max_chars);
        }
    }
    String::new()
}

fn image_urls(html: &str) -> String {
    let mut urls = Vec::new();
    let mut rest = html;
    while urls.len() < 20 {
        let Some(start) = rest.find("src=") else {
            break;
        };
        rest = &rest[start + 4..];
        let quote = rest.chars().next();
        if quote != Some('"') && quote != Some('\'') {
            continue;
        }
        rest = &rest[1..];
        let Some(end) = rest.find(quote.unwrap()) else {
            break;
        };
        let url = rest[..end].trim();
        rest = &rest[end + 1..];
        if url.starts_with("https://") && url.len() <= 2048 {
            urls.push(url);
        }
    }
    serde_json::to_string(&urls).unwrap_or_else(|_| "[]".into())
}

fn publication_kind(name: &str, hinted: &str) -> String {
    let hint = hinted.trim().to_ascii_lowercase();
    if hint == "weekly" || hint == "magazine" || name.contains('周') && name.contains('刊') {
        "magazine".into()
    } else {
        "feed".into()
    }
}

fn public_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with("https://") || raw.starts_with("http://") {
        clip(raw, 1000)
    } else {
        String::new()
    }
}

fn normalize_ts(raw: &str, fallback: &str) -> String {
    let text = raw.trim();
    if text.len() < 10 || text.as_bytes().get(4) != Some(&b'-') {
        return fallback.to_string();
    }
    if text.ends_with('Z') || text.ends_with('z') {
        format!("{}+00:00", &text[..text.len() - 1])
    } else {
        text.to_string()
    }
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let tod = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn simple_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

fn clip_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_must_match_and_missing_config_is_distinct() {
        assert_eq!(authorize(Some("Bearer secret"), "secret"), Ok(()));
        assert_eq!(authorize(Some("Bearer other"), "secret"), Err("令牌无效"));
        assert_eq!(authorize(None, ""), Err("未配置 XINCAI_INGEST_TOKEN"));
    }

    #[tokio::test]
    async fn pushed_article_is_saved_once_and_then_updated() {
        let path = std::env::temp_dir().join(format!(
            "vpush-xincai-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let body = json!({
            "group": "心裁",
            "articles": [
                {"sourceId": "si35", "sourceName": "财新周刊", "externalId": "a1", "title": "标题", "platform": "caixin", "topics": ["政经", "市场"], "html": "<p>你好</p>", "url": "https://example.com/a", "publishedAt": "2024-01-02T00:00:00Z"},
                {"title": "没有编号"},
                {"sourceId": "si35", "sourceName": "财新周刊", "externalId": "a2", "title": "越界", "url": "file:///etc/passwd", "html": "x"}
            ]
        });
        let saved = ingest(&db, &body).await.unwrap();
        assert_eq!(saved["accepted"], 2);
        assert_eq!(saved["skipped"], 1);
        let again = json!({"articles": [{"sourceId": "si35", "sourceName": "财新周刊", "externalId": "a1", "title": "新标题", "html": "<p>改过</p>", "url": "https://example.com/a"}]});
        let updated = ingest(&db, &again).await.unwrap();
        assert_eq!(updated["accepted"], 1);
        let user = db.user_by_username("admin").await.unwrap().unwrap();
        let listed = db.list_news(user.id, 0, "", false, 20, 0).await.unwrap();
        let items = listed["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items.iter().find(|item| item["title"] == "新标题").unwrap()["url"],
            "https://example.com/a"
        );
        assert!(items
            .iter()
            .any(|item| item["title"] == "越界" && item["url"] == ""));
        let article_id = items.iter().find(|item| item["title"] == "新标题").unwrap()["id"]
            .as_i64()
            .unwrap();
        let article = db.news_article(user.id, article_id).await.unwrap().unwrap();
        assert_eq!(article["content"], "<p>改过</p>");
        assert_eq!(article["content_html"], "<p>改过</p>");
        assert_eq!(article["source_platform"], "caixin");
        assert_eq!(article["topics"], json!(["政经", "市场"]));
        let figure = json!({"articles": [{"sourceId": "ft", "sourceName": "FT · 中国", "externalId": "f1", "title": "图", "platform": "ft", "html": "<figure><img src=\"https://img.example/a.jpg\"></figure>", "text": "正文 & 说明", "url": "https://example.com/f"}]});
        ingest(&db, &figure).await.unwrap();
        let listed = db.list_news(user.id, 0, "", false, 20, 0).await.unwrap();
        let figure_id = listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["title"] == "图")
            .unwrap()["id"]
            .as_i64()
            .unwrap();
        let figure = db.news_article(user.id, figure_id).await.unwrap().unwrap();
        let html = figure["content_html"].as_str().unwrap();
        assert!(html.contains("<img"));
        assert!(html.contains("正文 &amp; 说明"));
        assert_eq!(figure["source_platform"], "ft");
        assert!(figure["has_image"].as_bool().unwrap());
        let sources = db.admin_news_sources().await.unwrap();
        assert_eq!(sources[0]["kind"], "magazine");
        assert_eq!(sources[0]["feeds"][0]["enabled"], false);
        assert!(ingest(&db, &json!({"articles": []})).await.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn magazine_issue_keeps_cover_order_and_images() {
        let path = std::env::temp_dir().join(format!(
            "vpush-xincai-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let body = json!({
            "articles": [
                {"sourceId": "wk", "sourceName": "财新周刊", "externalId": "b", "title": "后一篇", "tocOrder": 2, "section": "市场", "html": "<p><img src=\"https://img.example/b.jpg\"></p>", "issueKey": "2026-30"},
                {"sourceId": "wk", "sourceName": "财新周刊", "externalId": "a", "title": "头一篇", "tocOrder": 1, "html": "<img src='https://img.example/a.jpg'>", "issue": {"key": "2026-30", "label": "第30期", "title": "封面故事", "cover": "https://img.example/cover.jpg"}}
            ]
        });
        ingest(&db, &body).await.unwrap();
        let user = db.user_by_username("admin").await.unwrap().unwrap();
        let source_id = db.admin_news_sources().await.unwrap()[0]["id"]
            .as_i64()
            .unwrap();
        let shelf = db.magazine(user.id, source_id).await.unwrap();
        let issue = &shelf["issues"][0];
        assert_eq!(issue["label"], "第30期");
        assert_eq!(issue["title"], "封面故事");
        assert_eq!(issue["cover"], "https://img.example/cover.jpg");
        assert_eq!(issue["articles"][0]["title"], "头一篇");
        assert_eq!(issue["articles"][0]["section"], "正文");
        assert_eq!(issue["articles"][1]["section"], "市场");
        let first = issue["articles"][0]["id"].as_i64().unwrap();
        assert_eq!(
            db.news_image_url(user.id, first, 0).await.unwrap(),
            "https://img.example/a.jpg"
        );
        assert!(db.news_image_url(user.id, first, 3).await.is_err());
        assert!(db.magazine(user.id, source_id + 9).await.is_err());
        let _ = std::fs::remove_file(&path);
    }
}
