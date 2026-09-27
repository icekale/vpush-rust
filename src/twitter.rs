//! X 时间线。只用后台保存的 Cookie 打网页 GraphQL，拉第一页。
//! queryId 每 6 小时从 x.com 的前端包读取。Cookie 通道走 Chrome 124 的 TLS 指纹。

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::db::Db;

const USER_TWEETS: &str = "T1x2zehUOKCWNpKwZCpnbg";
const USER_BY_NAME: &str = "Gb-d6r0vxPOADdG62OEBpQ";
const BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs=1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const FEATURES: &str = r#"{"responsive_web_graphql_timeline_navigation_enabled":true,"longform_notetweets_consumption_enabled":true,"view_counts_everywhere_api_enabled":true,"responsive_web_edit_tweet_api_enabled":true,"tweetypie_unmention_optimization_enabled":true,"freedom_of_speech_not_reach_fetch_enabled":true,"standardized_nudges_misinfo":true,"longform_notetweets_rich_text_read_enabled":true}"#;

struct Tweet {
    external_id: String,
    content: String,
    url: String,
    images: Vec<String>,
    published_at: String,
    published_unix: Option<i64>,
    reply: bool,
}

pub fn spawn(db: Db) {
    if !crate::xueqiu::fetch_enabled() {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Err(err) = poll(&db).await {
                tracing::warn!("X 抓取: {err}");
                let _ = db.note_platform("twitter", Some(&err)).await;
            } else {
                let _ = db.note_platform("twitter", None).await;
            }
            tokio::time::sleep(Duration::from_secs(crate::xueqiu::poll_wait(&db).await)).await;
        }
    });
}

async fn poll(db: &Db) -> Result<(), String> {
    let kols = db
        .kols_to_fetch("twitter")
        .await
        .map_err(|e| e.to_string())?;
    if kols.is_empty() {
        return Ok(());
    }
    let Some(cookie) = cookie(db).await? else {
        tracing::info!("X 抓取跳过：未配置含 auth_token 和 ct0 的 twitter_cookie");
        return Ok(());
    };
    let exit = crate::proxy_admin::acquire(db, "twitter").await?;
    let proxy = exit.as_ref().map(|item| item.url.clone());
    let proxy_id = exit.map(|item| item.id);
    refresh_ids(&cookie, proxy.as_deref()).await;
    for (id, name, external_id) in kols {
        let screen = screen_name(&external_id);
        if screen.is_empty() {
            tracing::warn!(kol = id, "无法识别 X 用户名");
            continue;
        }
        match pull(db, &cookie, id, &name, &screen, proxy.clone()).await {
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

async fn cookie(db: &Db) -> Result<Option<String>, String> {
    let saved = db
        .setting("twitter_cookie")
        .await
        .map_err(|e| e.to_string())?;
    let raw = saved
        .or_else(|| std::env::var("TWITTER_COOKIE").ok())
        .unwrap_or_default();
    let auth = pair(&raw, "auth_token");
    let ct0 = pair(&raw, "ct0");
    if auth.is_empty() || ct0.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("auth_token={auth}; ct0={ct0}; lang=zh-CN")))
}

async fn pull(
    db: &Db,
    cookie: &str,
    kol_id: i64,
    name: &str,
    screen: &str,
    proxy: Option<String>,
) -> Result<(), String> {
    let user_id = if screen.chars().all(|c| c.is_ascii_digit()) {
        screen.to_string()
    } else {
        let looked = graphql(
            cookie,
            "UserByScreenName",
            &current_id("UserByScreenName"),
            &json!({"screen_name": screen, "withSafetyModeUserFields": true}),
            proxy.as_deref(),
        )
        .await?;
        let result = &looked["data"]["user"]["result"];
        let id = field(result, "rest_id");
        if id.is_empty() {
            return Err(format!("X 未找到用户 {screen}"));
        }
        let avatar = result["avatar"]["image_url"]
            .as_str()
            .unwrap_or("")
            .replace("_normal", "_400x400");
        if avatar.starts_with("https://") {
            db.set_avatar(kol_id, &avatar)
                .await
                .map_err(|e| e.to_string())?;
        }
        id
    };
    let data = graphql(
        cookie,
        "UserTweets",
        &current_id("UserTweets"),
        &json!({
            "userId": user_id,
            "count": 20,
            "includePromotedContent": false,
            "withQuickPromoteEligibilityTweetFields": true,
            "withVoice": true,
            "withV2Timeline": true
        }),
        proxy.as_deref(),
    )
    .await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for tweet in tweets(&data, screen) {
        let Some(unix) = tweet.published_unix else {
            continue;
        };
        if now.saturating_sub(unix) > 36 * 3600 {
            continue;
        }
        if db
            .has_post("twitter", &tweet.external_id)
            .await
            .map_err(|e| e.to_string())?
        {
            continue;
        }
        let images = serde_json::to_string(&tweet.images).unwrap_or_else(|_| "[]".into());
        let kind = if tweet.reply { "reply" } else { "post" };
        db.save_fetched(
            kol_id,
            &tweet.external_id,
            &tweet.content.chars().take(80).collect::<String>(),
            &tweet.content,
            kind,
            &images,
            &tweet.url,
            &tweet.published_at,
        )
        .await
        .map_err(|e| e.to_string())?;
        if now.saturating_sub(unix) > 60 * 60
            || !db
                .should_push(kol_id, kind)
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
                platform: "twitter",
                post_type: kind,
                title: &tweet.content.chars().take(80).collect::<String>(),
                content: &tweet.content,
                url: &tweet.url,
                published_at: &tweet.published_at,
            },
        )
        .await;
    }
    Ok(())
}

async fn graphql(
    cookie: &str,
    operation: &str,
    query_id: &str,
    variables: &Value,
    proxy: Option<&str>,
) -> Result<Value, String> {
    post_graphql(cookie, operation, query_id, variables, proxy).await
}

fn browser() -> Result<wreq::Client, String> {
    static CLIENT: OnceLock<Result<wreq::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(chrome_client) {
        Ok(client) => Ok(client.clone()),
        Err(err) => Err(err.clone()),
    }
}

fn chrome_client() -> Result<wreq::Client, String> {
    wreq::Client::builder()
        .emulation(wreq_util::Emulation::Chrome124)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())
}

async fn post_graphql(
    cookie: &str,
    operation: &str,
    query_id: &str,
    variables: &Value,
    proxy: Option<&str>,
) -> Result<Value, String> {
    let ct0 = pair(cookie, "ct0");
    let features: Value = serde_json::from_str(FEATURES).unwrap_or(json!({}));
    let body = json!({"variables": variables, "features": features}).to_string();
    let url = format!("https://x.com/i/api/graphql/{query_id}/{operation}");
    let mut request = browser()?
        .post(&url)
        .query(&[
            ("variables", variables.to_string()),
            ("features", FEATURES.to_string()),
        ])
        .header("Authorization", format!("Bearer {BEARER}"))
        .header("Cookie", cookie)
        .header("x-csrf-token", &ct0)
        .header("x-twitter-active-user", "yes")
        .header("Content-Type", "application/json")
        .body(body);
    if let Some(proxy) = proxy {
        request = request.proxy(wreq::Proxy::all(proxy).map_err(|err| err.to_string())?);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if status == 400 || status == 404 {
        mark_ids_stale();
        return Err(format!("X {operation} HTTP {status}，queryId 可能已轮换"));
    }
    if status != 200 {
        return Err(format!("X {operation} HTTP {status}"));
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| "X 响应不是 JSON".to_string())?;
    if value
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
    {
        return Err(format!("X {operation} 返回错误"));
    }
    Ok(value)
}

fn tweets(data: &Value, screen: &str) -> Vec<Tweet> {
    let mut out = Vec::new();
    let Some(instructions) =
        data["data"]["user"]["result"]["timeline"]["timeline"]["instructions"].as_array()
    else {
        return out;
    };
    for instruction in instructions {
        let entries = if instruction["type"] == "TimelineAddEntries" {
            instruction["entries"].as_array()
        } else {
            None
        };
        let Some(entries) = entries else { continue };
        for entry in entries {
            if let Some(tweet) = tweet_from(entry, screen) {
                out.push(tweet);
            }
        }
    }
    out
}

fn tweet_from(entry: &Value, screen: &str) -> Option<Tweet> {
    let mut result = entry["content"]["itemContent"]["tweet_results"]["result"].clone();
    if result["tweet"].is_object() {
        result = result["tweet"].clone();
    }
    if result["legacy"].is_null() {
        return None;
    }
    let legacy = &result["legacy"];
    if legacy.get("retweeted_status_result").is_some() {
        return None;
    }
    let external_id = field(&result, "rest_id");
    let external_id = if external_id.is_empty() {
        field(legacy, "id_str")
    } else {
        external_id
    };
    if external_id.is_empty() {
        return None;
    }
    let mut text = field(legacy, "full_text");
    if text.is_empty() {
        text = field(legacy, "text");
    }
    let note = field(
        &result["note_tweet"]["note_tweet_results"]["result"],
        "text",
    );
    if note.chars().count() > text.chars().count() {
        text = note;
    }
    let images = photos(legacy);
    if text.is_empty() {
        text = if images.is_empty() {
            return None;
        } else {
            "图片".into()
        };
    }
    let (published_at, published_unix) = published(&field(legacy, "created_at"));
    let url = format!("https://x.com/{screen}/status/{external_id}");
    Some(Tweet {
        external_id,
        url,
        content: text,
        images,
        published_at,
        published_unix,
        reply: !field(legacy, "in_reply_to_status_id_str").is_empty(),
    })
}

fn photos(legacy: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(media) = legacy["extended_entities"]["media"].as_array() else {
        return out;
    };
    for item in media {
        if item["type"] != "photo" {
            continue;
        }
        let url = field(item, "media_url_https");
        if url.starts_with("https://") && !out.contains(&url) && out.len() < 4 {
            out.push(url);
        }
    }
    out
}

fn published(raw: &str) -> (String, Option<i64>) {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() == 6 {
        if let Some(unix) = twitter_unix(&parts) {
            let beijing = unix + 8 * 3600;
            let days = beijing.div_euclid(86400);
            let sod = beijing.rem_euclid(86400);
            let (y, m, d) = civil_from_days(days);
            return (
                format!(
                    "{y:04}-{m:02}-{d:02} {:02}:{:02}",
                    sod / 3600,
                    (sod % 3600) / 60
                ),
                Some(unix),
            );
        }
    }
    (raw.to_string(), None)
}

fn twitter_unix(parts: &[&str]) -> Option<i64> {
    let month = month_num(parts[1])?;
    let day: u32 = parts[2].parse().ok()?;
    let year: i32 = parts[5].parse().ok()?;
    let clock: Vec<&str> = parts[3].split(':').collect();
    let hour: i64 = clock.first()?.parse().ok()?;
    let minute: i64 = clock.get(1)?.parse().ok()?;
    let second: i64 = clock.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let offset = zone_secs(parts[4])?;
    Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second - offset)
}

fn month_num(name: &str) -> Option<u32> {
    Some(match name {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

fn zone_secs(raw: &str) -> Option<i64> {
    let sign = match raw.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return None,
    };
    if raw.len() < 5 {
        return None;
    }
    let hour: i64 = raw[1..3].parse().ok()?;
    let minute: i64 = raw[3..5].parse().ok()?;
    Some(sign * (hour * 3600 + minute * 60))
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

fn screen_name(raw: &str) -> String {
    let text = raw.trim().trim_end_matches('/');
    let seg = text
        .rsplit('/')
        .next()
        .unwrap_or(text)
        .trim_start_matches('@');
    seg.chars().take(15).collect()
}

fn pair(cookie: &str, name: &str) -> String {
    cookie
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name).then(|| value.to_string())
        })
        .unwrap_or_default()
}

struct Ids {
    tweets: String,
    by_name: String,
    loaded: u64,
    error_until: u64,
}

fn id_store() -> &'static Mutex<Ids> {
    static IDS: OnceLock<Mutex<Ids>> = OnceLock::new();
    IDS.get_or_init(|| {
        Mutex::new(Ids {
            tweets: USER_TWEETS.to_string(),
            by_name: USER_BY_NAME.to_string(),
            loaded: 0,
            error_until: 0,
        })
    })
}

fn current_id(operation: &str) -> String {
    let guard = id_store().lock().unwrap_or_else(|err| err.into_inner());
    if operation == "UserByScreenName" {
        guard.by_name.clone()
    } else {
        guard.tweets.clone()
    }
}

fn mark_ids_stale() {
    let mut guard = id_store().lock().unwrap_or_else(|err| err.into_inner());
    guard.loaded = 0;
    guard.error_until = now_secs() + 300;
}

async fn refresh_ids(cookie: &str, proxy: Option<&str>) {
    let now = now_secs();
    {
        let guard = id_store().lock().unwrap_or_else(|err| err.into_inner());
        if guard.loaded != 0 && now.saturating_sub(guard.loaded) < 6 * 3600 {
            return;
        }
        if now < guard.error_until {
            return;
        }
    }
    match fetch_ids(cookie, proxy).await {
        Ok((tweets, by_name)) if tweets.is_some() || by_name.is_some() => {
            let mut guard = id_store().lock().unwrap_or_else(|err| err.into_inner());
            if let Some(tweets) = tweets {
                guard.tweets = tweets;
            }
            if let Some(by_name) = by_name {
                guard.by_name = by_name;
            }
            guard.loaded = now;
        }
        _ => {
            let mut guard = id_store().lock().unwrap_or_else(|err| err.into_inner());
            guard.error_until = now + 300;
            tracing::warn!("X queryId 提取失败，5 分钟后重试");
        }
    }
}

async fn fetch_ids(
    cookie: &str,
    proxy: Option<&str>,
) -> Result<(Option<String>, Option<String>), String> {
    let page = get_html("https://x.com/", cookie, proxy).await?;
    let Some(url) = bundle_url(&page) else {
        return Ok((None, None));
    };
    let bundle = get_html(&url, cookie, proxy).await?;
    Ok((
        find_query_id(&bundle, "UserTweets"),
        find_query_id(&bundle, "UserByScreenName"),
    ))
}

async fn get_html(url: &str, cookie: &str, proxy: Option<&str>) -> Result<String, String> {
    if !allowed_asset(url) {
        return Err("地址不允许".into());
    }
    let mut request = browser()?.get(url).header("Cookie", cookie).header(
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    );
    if let Some(proxy) = proxy {
        request = request.proxy(wreq::Proxy::all(proxy).map_err(|err| err.to_string())?);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    let text = response.text().await.map_err(|err| err.to_string())?;
    Ok(text.chars().take(8_000_000).collect())
}

fn allowed_asset(url: &str) -> bool {
    url == "https://x.com/"
        || (url.starts_with("https://abs.twimg.com/responsive-web/client-web/main.")
            && url.ends_with(".js")
            && !url.contains("..")
            && url.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/' | b':')
            }))
}

fn bundle_url(page: &str) -> Option<String> {
    let marker = "https://abs.twimg.com/responsive-web/client-web/main.";
    let start = page.find(marker)?;
    let rest = &page[start..];
    let end = rest.find(".js\"").or_else(|| rest.find(".js'"))?;
    let url = &rest[..end + 3];
    allowed_asset(url).then(|| url.to_string())
}

fn find_query_id(text: &str, operation: &str) -> Option<String> {
    let needle = format!("operationName:\"{operation}\"");
    let mut from = 0;
    while let Some(pos) = text[from..].find(&needle) {
        let abs = from + pos;
        let window = &text[abs.saturating_sub(320)..abs];
        if let Some(id) = query_in(window) {
            return Some(id);
        }
        from = abs + needle.len();
    }
    None
}

fn query_in(window: &str) -> Option<String> {
    let marker = "queryId:\"";
    let mut search = window;
    while let Some(start) = search.rfind(marker) {
        let value_at = start + marker.len();
        let rest = &window[value_at..];
        let end = rest.find('"')?;
        let id = &rest[..end];
        let between = &rest[end + 1..];
        if query_ok(id) && !between.contains('}') {
            return Some(id.to_string());
        }
        search = &window[..start];
    }
    None
}

fn query_ok(id: &str) -> bool {
    (8..=80).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    fn reads_a_tweet_and_skips_retweets() {
        let data = json!({"data": {"user": {"result": {"timeline": {"timeline": {"instructions": [{
            "type": "TimelineAddEntries",
            "entries": [
                {"content": {"itemContent": {"tweet_results": {"result": {
                    "rest_id": "42",
                    "legacy": {
                        "full_text": "你好",
                        "created_at": "Wed Oct 10 20:19:24 +0000 2012",
                        "extended_entities": {"media": [{"type": "photo", "media_url_https": "https://pbs.twimg.com/a.jpg"}, {"type": "video", "media_url_https": "https://pbs.twimg.com/v.mp4"}]}
                    }
                }}}}},
                {"content": {"itemContent": {"tweet_results": {"result": {
                    "rest_id": "7",
                    "legacy": {"full_text": "转", "retweeted_status_result": {"result": {}}}
                }}}}}
            ]
        }]}}}}}});
        let rows = tweets(&data, "alice");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "你好");
        assert_eq!(rows[0].published_at, "2012-10-11 04:19");
        assert_eq!(
            rows[0].images,
            vec!["https://pbs.twimg.com/a.jpg".to_string()]
        );
        assert_eq!(rows[0].url, "https://x.com/alice/status/42");
        assert_eq!(screen_name("https://x.com/alice"), "alice");
        assert_eq!(pair("auth_token=a; ct0=b", "ct0"), "b");
        let page = r#"<script src="https://abs.twimg.com/responsive-web/client-web/main.abc123.js"></script>"#;
        assert_eq!(
            bundle_url(page).as_deref(),
            Some("https://abs.twimg.com/responsive-web/client-web/main.abc123.js")
        );
        let bundle = r#"{queryId:"NEWUSER01",operationName:"UserTweets",}{queryId:"OLD",}{queryId:"NAMEID001",operationName:"UserByScreenName"}"#;
        assert_eq!(
            find_query_id(bundle, "UserTweets").as_deref(),
            Some("NEWUSER01")
        );
        assert_eq!(
            find_query_id(bundle, "UserByScreenName").as_deref(),
            Some("NAMEID001")
        );
        assert!(!allowed_asset("https://evil.example/main.js"));
    }

    #[test]
    fn chrome124_tls_hello_differs_from_firefox() {
        let chrome = tls_hello(wreq_util::Emulation::Chrome124);
        let firefox = tls_hello(wreq_util::Emulation::Firefox139);
        assert_eq!(
            chrome.first().copied(),
            Some(0x16),
            "chrome hello is not TLS"
        );
        assert_eq!(
            firefox.first().copied(),
            Some(0x16),
            "firefox hello is not TLS"
        );
        assert_ne!(chrome, firefox);
        assert!(chrome.windows(2).any(|part| part == b"h2"));
    }

    fn tls_hello(profile: wreq_util::Emulation) -> Vec<u8> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let reader = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut buf = vec![0u8; 8192];
            let n = std::io::Read::read(&mut sock, &mut buf).unwrap_or(0);
            buf.truncate(n);
            buf
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let client = wreq::Client::builder()
                .emulation(profile)
                .no_proxy()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            let _ = client
                .get(format!("https://127.0.0.1:{port}/"))
                .send()
                .await;
        });
        reader.join().unwrap()
    }
}
