use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::db::Db;

const COOLDOWN: u64 = 30 * 60;
static MYMEMORY_SKIP_UNTIL: AtomicU64 = AtomicU64::new(0);

pub async fn for_new_post(
    db: &Db,
    platform: &str,
    external_id: &str,
    title: &str,
    content: &str,
    proxy: Option<&str>,
) -> (String, String, String, String) {
    if platform != "twitter" && platform != "truth" {
        return owned(title, content, "", "");
    }
    let enabled = db
        .setting("config_translate_twitter_content")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some("1");
    if !enabled || already_chinese(&quoted(content)) {
        return owned(title, content, "", "");
    }
    let tweet_id = (platform == "twitter" && quoted(content) == content)
        .then(|| tweet_id(external_id))
        .flatten();
    match text(db, content, tweet_id.as_deref(), proxy).await {
        Ok(translated)
            if !translated.trim().is_empty()
                && !collapsed(&translated, content)
                && translated.trim() != content.trim() =>
        {
            let title_zh: String = translated
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(80)
                .collect();
            (title_zh, translated, title.to_string(), content.to_string())
        }
        Ok(_) => owned(title, content, title, content),
        Err(err) => {
            tracing::warn!("X 内容翻译失败 post={external_id} err={err}");
            owned(title, content, "", "")
        }
    }
}

pub async fn text(
    db: &Db,
    source: &str,
    tweet_id: Option<&str>,
    proxy: Option<&str>,
) -> Result<String, String> {
    let source = source.trim();
    if source.is_empty() || already_chinese(&quoted(source)) {
        return Ok(source.to_string());
    }
    let mut errors = Vec::new();
    if let Some((auth, ct0)) = x_cookie(db).await {
        let id = tweet_id.filter(|id| !id.is_empty());
        let payload = if let Some(id) = id {
            serde_json::json!({
                "content_type": "POST",
                "id": id,
                "dst_lang": "zh-cn",
                "include_polls": true
            })
        } else {
            serde_json::json!({
                "content_type": "TEXT",
                "text": source.chars().take(2000).collect::<String>(),
                "dst_lang": "zh-cn"
            })
        };
        match post_json(
            "https://api.x.com/2/grok/translation.json",
            &payload.to_string(),
            &[
                (
                    "Authorization",
                    format!("Bearer {}", crate::twitter::BEARER),
                ),
                ("Content-Type", "application/json".into()),
                (
                    "Cookie",
                    format!("auth_token={auth}; ct0={ct0}; lang=zh-CN"),
                ),
                ("x-csrf-token", ct0),
                ("x-twitter-active-user", "yes".into()),
            ],
            proxy,
        )
        .await
        {
            Ok(body) => {
                let translated = grok_text(&body);
                if !translated.is_empty() && !collapsed(&translated, source) {
                    return Ok(translated);
                }
            }
            Err(err) => errors.push(format!("x_translate: {err}")),
        }
    }
    match post_json(
        "https://edge.microsoft.com/translate/translatetext?from=&to=zh-Hans&isEnterpriseClient=false",
        &serde_json::json!([source.chars().take(2000).collect::<String>()]).to_string(),
        &[("Content-Type", "application/json".into())],
        proxy,
    )
    .await
    {
        Ok(body) => {
            if let Some(translated) = edge_text(&body) {
                if !collapsed(&translated, source) {
                    return Ok(translated);
                }
            }
        }
        Err(err) => errors.push(format!("edge_translate: {err}")),
    }
    if source.chars().count() > 500 || now() < MYMEMORY_SKIP_UNTIL.load(Ordering::Relaxed) {
        return if errors.is_empty() {
            Ok(source.to_string())
        } else {
            Err(errors.join("; "))
        };
    }
    match get_text(
        &format!(
            "https://api.mymemory.translated.net/get?q={}&langpair=en%7Czh-CN",
            urlencoding_q(&source.chars().take(500).collect::<String>())
        ),
        proxy,
    )
    .await
    {
        Ok((status, body)) => {
            if status == 429 {
                MYMEMORY_SKIP_UNTIL.store(now() + COOLDOWN, Ordering::Relaxed);
                return Ok(source.to_string());
            }
            if let Some(translated) = mymemory_text(&body) {
                if !collapsed(&translated, source) {
                    return Ok(translated);
                }
            }
        }
        Err(err) => errors.push(format!("mymemory: {err}")),
    }
    if errors.is_empty() {
        Ok(source.to_string())
    } else {
        Err(errors.join("; "))
    }
}

fn owned(
    title: &str,
    content: &str,
    title_src: &str,
    content_src: &str,
) -> (String, String, String, String) {
    (
        title.to_string(),
        content.to_string(),
        title_src.to_string(),
        content_src.to_string(),
    )
}

async fn x_cookie(db: &Db) -> Option<(String, String)> {
    let saved = db.setting("twitter_cookie").await.ok().flatten();
    let raw = saved.or_else(|| std::env::var("TWITTER_COOKIE").ok())?;
    let auth = pair(&raw, "auth_token");
    let ct0 = pair(&raw, "ct0");
    if auth.is_empty() || ct0.is_empty() {
        None
    } else {
        Some((auth, ct0))
    }
}

fn pair(cookie: &str, name: &str) -> String {
    cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default()
}

fn tweet_id(external_id: &str) -> Option<String> {
    let id = external_id.trim();
    if !id.is_empty() && id.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(id.to_string());
    }
    let marker = ["/status/", "status/"]
        .into_iter()
        .find(|m| id.contains(m))?;
    let rest = id.split(marker).nth(1)?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    (!digits.is_empty()).then_some(digits)
}

pub(crate) fn quoted(text: &str) -> String {
    if let Some((head, _)) = text.split_once("\n\nRT @") {
        if !head.trim().is_empty() {
            return head.to_string();
        }
    }
    text.to_string()
}

pub(crate) fn already_chinese(text: &str) -> bool {
    let cjk = text
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    if cjk < 4 {
        return false;
    }
    let stripped = strip_links(text);
    let foreign = stripped
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .count()
        + stripped
            .chars()
            .filter(|ch| ('\u{3040}'..='\u{30ff}').contains(ch))
            .count();
    cjk * 4 >= foreign * 3
}

pub(crate) fn collapsed(translated: &str, source: &str) -> bool {
    let text = translated.trim();
    let original = source.trim();
    if original.is_empty() || text == original {
        return false;
    }
    if text.is_empty() || squeeze(text) == squeeze(original) {
        return true;
    }
    if already_chinese(original)
        && squeeze(text).chars().count() * 2 < squeeze(original).chars().count()
    {
        return true;
    }
    if text.chars().all(|ch| !ch.is_alphanumeric()) && original.chars().count() > 3 {
        return true;
    }
    if text.chars().count() <= 4 && original.chars().count() >= 20 {
        return true;
    }
    let author = quoted(original);
    if author != original && original.chars().count() >= 40 {
        let cap = (author.chars().count() + 8).clamp(16, 24);
        return text.chars().count() <= cap;
    }
    false
}

fn squeeze(text: &str) -> String {
    strip_links(text)
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
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

pub(crate) fn grok_text(body: &str) -> String {
    let mut found = String::new();
    let mut rest = body;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        let Some(Ok(obj)) = stream.next() else {
            break;
        };
        let used = stream.byte_offset();
        if used == 0 {
            break;
        }
        rest = &rest[used..];
        let text = obj
            .get("result")
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !text.is_empty() {
            found = text.to_string();
        }
    }
    found
}

pub(crate) fn edge_text(body: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let text = parsed
        .as_array()?
        .first()?
        .get("translations")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()?
        .trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn mymemory_text(body: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let text = parsed
        .get("responseData")?
        .get("translatedText")?
        .as_str()?
        .trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn urlencoding_q(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn post_json(
    url: &str,
    body: &str,
    headers: &[(&str, String)],
    proxy: Option<&str>,
) -> Result<String, String> {
    let mut request = crate::twitter::browser()?.post(url).body(body.to_string());
    for (key, value) in headers {
        request = request.header(*key, value);
    }
    send(request, proxy).await
}

async fn get_text(url: &str, proxy: Option<&str>) -> Result<(u16, String), String> {
    let request = crate::twitter::browser()?.get(url);
    let response = with_proxy(request, proxy)?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|err| err.to_string())?;
    Ok((status, text))
}

async fn send(request: wreq::RequestBuilder, proxy: Option<&str>) -> Result<String, String> {
    let response = with_proxy(request, proxy)?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    Ok(text)
}

fn with_proxy(
    request: wreq::RequestBuilder,
    proxy: Option<&str>,
) -> Result<wreq::RequestBuilder, String> {
    if let Some(proxy) = proxy {
        Ok(request.proxy(wreq::Proxy::all(proxy).map_err(|err| err.to_string())?))
    } else {
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_grok_stream_and_edge() {
        let body = r#"{"result":{"text":""}} {"result":{"text":"收入增长。"}}"#;
        assert_eq!(grok_text(body), "收入增长。");
        assert_eq!(
            edge_text(r#"[{"translations":[{"text":"公司上调指引"}]}]"#).as_deref(),
            Some("公司上调指引")
        );
        assert!(already_chinese(
            "这是一段已经写成中文的帖子，不该再送去翻译。"
        ));
        assert!(!already_chinese(
            "Revenue grew and the company raised guidance today."
        ));
        assert!(collapsed(
            "…",
            "Revenue grew and the company raised its full-year guidance."
        ));
        assert_eq!(tweet_id("12345").as_deref(), Some("12345"));
        assert_eq!(
            tweet_id("https://x.com/alice/status/9988").as_deref(),
            Some("9988")
        );
    }
}
