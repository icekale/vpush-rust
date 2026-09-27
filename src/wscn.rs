//! 华尔街见闻 7x24 快讯。只代理首页，不入库。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const LIVES_URL: &str = "https://api-one-wscn.awtmt.com/apiv1/content/lives";
const TTL: Duration = Duration::from_secs(15);

struct Slot {
    at: Instant,
    page: Value,
}

fn cache() -> &'static Mutex<HashMap<String, Slot>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn live(cursor: &str, limit: i64, since_id: i64) -> Result<Value, String> {
    let key = format!("{cursor}:{limit}");
    if let Some(page) = fresh(&key) {
        return Ok(filter_since(page, since_id));
    }
    let cursor = cursor.to_string();
    let fetched = tokio::task::spawn_blocking(move || fetch(&cursor, limit))
        .await
        .map_err(|e| e.to_string())?;
    match fetched {
        Ok(page) => {
            store(&key, &page);
            Ok(filter_since(page, since_id))
        }
        Err(err) => {
            tracing::warn!("华尔街见闻快讯: {err}");
            stale(&key)
                .map(|page| filter_since(page, since_id))
                .ok_or_else(|| "快讯源暂时不可用".into())
        }
    }
}

fn fresh(key: &str) -> Option<Value> {
    let guard = cache().lock().unwrap_or_else(|err| err.into_inner());
    let slot = guard.get(key)?;
    (slot.at.elapsed() <= TTL).then(|| slot.page.clone())
}

fn stale(key: &str) -> Option<Value> {
    let guard = cache().lock().unwrap_or_else(|err| err.into_inner());
    guard.get(key).map(|slot| slot.page.clone())
}

fn store(key: &str, page: &Value) {
    let mut guard = cache().lock().unwrap_or_else(|err| err.into_inner());
    guard.insert(
        key.to_string(),
        Slot {
            at: Instant::now(),
            page: page.clone(),
        },
    );
    if guard.len() > 64 {
        let oldest = guard
            .iter()
            .min_by_key(|(_, slot)| slot.at)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            guard.remove(&oldest);
        }
    }
}

fn fetch(cursor: &str, limit: i64) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(8))
        .build();
    let mut req = agent
        .get(LIVES_URL)
        .query("channel", "global-channel")
        .query("limit", &limit.to_string())
        .set(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        );
    if !cursor.is_empty() {
        req = req.query("cursor", cursor);
    }
    let text = match req.call() {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, resp)) => {
            let _ = resp.into_string();
            return Err(format!("HTTP {code}"));
        }
        Err(err) => return Err(err.to_string()),
    };
    let payload: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    page_from(&payload)
}

pub(crate) fn page_from(payload: &Value) -> Result<Value, String> {
    if payload.get("code") != Some(&json!(20000)) {
        let msg = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("WSCN API 错误");
        return Err(msg.to_string());
    }
    let data = &payload["data"];
    let items = data["items"]
        .as_array()
        .map(|rows| rows.iter().filter_map(item_from).collect::<Vec<_>>())
        .unwrap_or_default();
    let polling = data["polling_cursor"]
        .as_i64()
        .or_else(|| data["polling_cursor"].as_str().and_then(|s| s.parse().ok()))
        .or_else(|| items.first().and_then(|item| item["id"].as_i64()))
        .unwrap_or(0);
    Ok(json!({
        "items": items,
        "next_cursor": data["next_cursor"].as_str().unwrap_or("").trim(),
        "polling_cursor": polling,
    }))
}

fn item_from(raw: &Value) -> Option<Value> {
    let id = raw["id"]
        .as_i64()
        .or_else(|| raw["id"].as_str().and_then(|s| s.parse().ok()))?;
    let ts = raw["display_time"].as_i64().unwrap_or(0);
    let content = raw["content"]
        .as_str()
        .or_else(|| raw["content_text"].as_str())
        .unwrap_or("");
    let url = raw["uri"].as_str().unwrap_or("").trim();
    let url = if url.is_empty() {
        format!("https://wallstreetcn.com/livenews/{id}")
    } else {
        url.to_string()
    };
    Some(json!({
        "id": id,
        "score": raw["score"].as_i64().unwrap_or(1),
        "highlight_title": raw["highlight_title"].as_str().unwrap_or("").trim(),
        "body": plain(content),
        "published_at": beijing_iso(ts),
        "url": url,
    }))
}

fn filter_since(mut page: Value, since_id: i64) -> Value {
    if since_id <= 0 {
        return page;
    }
    if let Some(items) = page.get_mut("items").and_then(Value::as_array_mut) {
        items.retain(|item| item["id"].as_i64().unwrap_or(0) > since_id);
    }
    page
}

fn plain(content: &str) -> String {
    let mut text = String::new();
    let mut rest = content;
    while !rest.is_empty() {
        let Some(start) = rest.find('<') else {
            text.push_str(rest);
            break;
        };
        text.push_str(&rest[..start]);
        let end = rest[start..]
            .find('>')
            .map(|n| start + n)
            .unwrap_or(rest.len() - 1);
        let tag = rest[start..=end].to_ascii_lowercase();
        if tag.starts_with("<br") || tag.starts_with("</p") {
            text.push('\n');
        }
        rest = if end + 1 < rest.len() {
            &rest[end + 1..]
        } else {
            ""
        };
    }
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn beijing_iso(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    let local = ts + 8 * 3600;
    let (year, month, day) = civil_from_days(local.div_euclid(86400));
    let sod = local.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+08:00",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_live_item_and_filters_since() {
        let payload = json!({
            "code": 20000,
            "data": {
                "items": [{
                    "id": 9,
                    "display_time": 1350000000,
                    "content": "甲<br>乙 &amp; 丙",
                    "score": 2,
                    "highlight_title": " 标题 ",
                    "uri": "https://example/a"
                }],
                "next_cursor": "8",
                "polling_cursor": "9"
            }
        });
        let page = page_from(&payload).unwrap();
        assert_eq!(page["items"][0]["body"], "甲\n乙 & 丙");
        assert_eq!(page["items"][0]["highlight_title"], "标题");
        assert_eq!(
            page["items"][0]["published_at"],
            "2012-10-12T08:00:00+08:00"
        );
        assert_eq!(page["next_cursor"], "8");
        assert_eq!(page["polling_cursor"], 9);
        assert_eq!(
            filter_since(page.clone(), 9)["items"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(filter_since(page, 8)["items"][0]["id"], 9);
        assert!(page_from(&json!({"code": 500})).is_err());
    }
}
