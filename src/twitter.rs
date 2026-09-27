//! X 时间线。只用后台保存的 Cookie 打网页 GraphQL，拉第一页。
//! queryId 每 6 小时从 x.com 的前端包读取。Cookie 通道走 Chrome 124 的 TLS 指纹。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use serde_json::{json, Value};

use crate::db::Db;

const USER_TWEETS: &str = "T1x2zehUOKCWNpKwZCpnbg";
const USER_BY_NAME: &str = "Gb-d6r0vxPOADdG62OEBpQ";
const BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs=1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const FEATURES: &str = r#"{"rweb_video_screen_enabled":false,"rweb_tipjar_consumption_enabled":true,"responsive_web_graphql_exclude_directive_enabled":true,"verified_phone_label_enabled":false,"creator_subscriptions_tweet_preview_api_enabled":true,"responsive_web_graphql_timeline_navigation_enabled":true,"responsive_web_graphql_skip_user_profile_image_extensions_enabled":false,"tweetypie_unmention_optimization_enabled":true,"responsive_web_edit_tweet_api_enabled":true,"graphql_is_translatable_rweb_tweet_is_translatable_enabled":true,"view_counts_everywhere_api_enabled":true,"longform_notetweets_consumption_enabled":true,"responsive_web_twitter_article_tweet_consumption_enabled":false,"tweet_awards_web_tipping_enabled":false,"freedom_of_speech_not_reach_fetch_enabled":true,"standardized_nudges_misinfo":true,"tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled":true,"rweb_video_timestamps_enabled":true,"longform_notetweets_rich_text_read_enabled":true,"responsive_web_enhance_cards_enabled":false}"#;
const APP_CONSUMER_KEY: &str = "3nVuSoBZnx6U4vzUxf5w";
const APP_CONSUMER_SECRET: &str = "Bcs59EFbbsdF6Sl9Ng71smgStWEGwXXKSjYvPVt7qys";
const APP_UA: &str = "TwitterAndroid/12.27.1 (Android 14; com.twitter.android)";
const CHANNEL_COOLDOWN: u64 = 900;

type HmacSha1 = Hmac<Sha1>;

struct AppAuth {
    token: String,
    secret: String,
}

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
    let cookie = cookie(db).await?;
    let app = app_auth(db).await?;
    if cookie.is_none() && app.is_none() {
        tracing::info!("X 抓取跳过：未配置 Cookie 或 App 凭证");
        return Ok(());
    }
    let exit = crate::proxy_admin::acquire(db, "twitter").await?;
    let proxy = exit.as_ref().map(|item| item.url.clone());
    let proxy_id = exit.map(|item| item.id);
    if let Some(cookie) = &cookie {
        refresh_ids(cookie, proxy.as_deref()).await;
    }
    let has_app = app.is_some();
    let has_cookie = cookie.is_some();
    for (id, name, external_id) in kols {
        if channels_cooling(has_app, has_cookie) {
            tracing::info!("X 通道正在 429 冷却，剩余账号跳过");
            break;
        }
        let screen = screen_name(&external_id);
        if screen.is_empty() {
            tracing::warn!(kol = id, "无法识别 X 用户名");
            continue;
        }
        let prefer = channel_for(id, has_app, has_cookie);
        match pull(
            db,
            cookie.as_deref(),
            app.as_ref(),
            prefer,
            id,
            &name,
            &screen,
            proxy.clone(),
        )
        .await
        {
            Ok(()) => {
                crate::proxy_admin::note(db, proxy_id, true, "").await;
                let _ = db.note_kol_fetch(id, None).await;
            }
            Err(err) => {
                crate::proxy_admin::note(db, proxy_id, false, &err).await;
                let _ = db.note_kol_fetch(id, Some(&err)).await;
                tracing::warn!(kol = id, "{err}");
                if err.contains("429") {
                    break;
                }
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
    cookie: Option<&str>,
    app: Option<&AppAuth>,
    prefer: &str,
    kol_id: i64,
    name: &str,
    screen: &str,
    proxy: Option<String>,
) -> Result<(), String> {
    let user_id = if screen.chars().all(|c| c.is_ascii_digit()) {
        screen.to_string()
    } else if let Some(id) = cached_user(screen) {
        id
    } else {
        let (id, avatar) = resolve_user(cookie, app, prefer, screen, proxy.as_deref()).await?;
        remember_user(screen, &id);
        if avatar.starts_with("https://") {
            db.set_avatar(kol_id, &avatar)
                .await
                .map_err(|e| e.to_string())?;
        }
        id
    };
    let data = graphql(
        cookie,
        app,
        prefer,
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

async fn resolve_user(
    cookie: Option<&str>,
    app: Option<&AppAuth>,
    prefer: &str,
    screen: &str,
    proxy: Option<&str>,
) -> Result<(String, String), String> {
    let mut prefer = prefer;
    if prefer == "cookie" {
        if let Some(cookie) = cookie {
            match typeahead(cookie, screen, proxy).await {
                Ok(Some(found)) => return Ok(found),
                Ok(None) => {}
                Err(err) if err.contains("429") => {
                    mark_cooling("cookie");
                    if app.is_none() {
                        return Err(err);
                    }
                    prefer = "app";
                }
                Err(err) => tracing::warn!("X typeahead: {err}"),
            }
        }
    }
    let looked = graphql(
        cookie,
        app,
        prefer,
        "UserByScreenName",
        &current_id("UserByScreenName"),
        &json!({"screen_name": screen, "withSafetyModeUserFields": true}),
        proxy,
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
    Ok((id, avatar))
}

async fn graphql(
    cookie: Option<&str>,
    app: Option<&AppAuth>,
    prefer: &str,
    operation: &str,
    query_id: &str,
    variables: &Value,
    proxy: Option<&str>,
) -> Result<Value, String> {
    let mut order = vec![prefer];
    let other = if prefer == "app" { "cookie" } else { "app" };
    let other_ready = match other {
        "app" => app.is_some() && !cooling("app"),
        "cookie" => cookie.is_some() && !cooling("cookie"),
        _ => false,
    };
    if other_ready {
        order.push(other);
    }
    let mut last = String::new();
    for (index, channel) in order.iter().enumerate() {
        let result = if *channel == "app" {
            post_app(
                app.ok_or_else(|| "X 未配置 App 凭证".to_string())?,
                operation,
                query_id,
                variables,
                proxy,
            )
            .await
        } else {
            post_graphql(cookie.unwrap_or(""), operation, query_id, variables, proxy).await
        };
        match result {
            Ok(value) => return Ok(value),
            Err(err) if err.contains("429") => {
                mark_cooling(channel);
                tracing::info!("X {channel} 通道撞 429，operation={operation}");
                last = err;
                if index + 1 == order.len() {
                    return Err(last);
                }
            }
            Err(err) => return Err(err),
        }
    }
    Err(last)
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

async fn post_app(
    auth: &AppAuth,
    operation: &str,
    query_id: &str,
    variables: &Value,
    proxy: Option<&str>,
) -> Result<Value, String> {
    let variables_json = variables.to_string();
    let url = format!("https://api.x.com/graphql/{query_id}/{operation}");
    let query = format!(
        "features={}&variables={}",
        oauth_escape(FEATURES),
        oauth_escape(&variables_json)
    );
    let authorization = oauth1_authorization(
        "POST",
        &url,
        &[
            ("features", FEATURES),
            ("variables", variables_json.as_str()),
        ],
        &auth.token,
        &auth.secret,
        &nonce(),
        &now_secs().to_string(),
    );
    let features: Value = serde_json::from_str(FEATURES).unwrap_or(json!({}));
    let body = json!({"variables": variables, "features": features}).to_string();
    let mut request = browser()?
        .post(format!("{url}?{query}"))
        .header("Authorization", authorization)
        .header("User-Agent", APP_UA)
        .header("x-twitter-active-user", "yes")
        .header("x-twitter-client-language", "zh-CN")
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
        return Err(format!(
            "X app {operation} HTTP {status}，queryId 可能已轮换"
        ));
    }
    if status != 200 {
        return Err(format!("X app {operation} HTTP {status}"));
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| "X 响应不是 JSON".to_string())?;
    if value
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
    {
        return Err(format!("X app {operation} 返回错误"));
    }
    Ok(value)
}

async fn typeahead(
    cookie: &str,
    screen: &str,
    proxy: Option<&str>,
) -> Result<Option<(String, String)>, String> {
    let url = format!(
        "https://x.com/i/api/1.1/search/typeahead.json?q={}&result_type=users",
        oauth_escape(screen)
    );
    let ct0 = pair(cookie, "ct0");
    let mut request = browser()?
        .get(&url)
        .header("Authorization", format!("Bearer {BEARER}"))
        .header("Cookie", cookie)
        .header("x-csrf-token", &ct0)
        .header("x-twitter-active-user", "yes");
    if let Some(proxy) = proxy {
        request = request.proxy(wreq::Proxy::all(proxy).map_err(|err| err.to_string())?);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    if status == 429 {
        return Err("X typeahead HTTP 429".into());
    }
    if status != 200 {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(&response.text().await.map_err(|err| err.to_string())?)
        .map_err(|_| "X typeahead 不是 JSON".to_string())?;
    let Some(users) = value["users"].as_array() else {
        return Ok(None);
    };
    for user in users {
        let name = user["screen_name"].as_str().unwrap_or("");
        if !name.eq_ignore_ascii_case(screen) {
            continue;
        }
        let id = user["id_str"].as_str().unwrap_or("").to_string();
        if id.is_empty() {
            return Ok(None);
        }
        let avatar = user["profile_image_url_https"]
            .as_str()
            .unwrap_or("")
            .replace("_normal", "_400x400");
        return Ok(Some((id, avatar)));
    }
    Ok(None)
}

async fn app_auth(db: &Db) -> Result<Option<AppAuth>, String> {
    let mode = setting_or_env(db, "x_auth_mode", "X_AUTH_MODE").await?;
    if mode != "oauth1" {
        return Ok(None);
    }
    let token = setting_or_env(db, "x_oauth_token", "X_OAUTH_TOKEN").await?;
    let secret = setting_or_env(db, "x_oauth_token_secret", "X_OAUTH_TOKEN_SECRET").await?;
    if token.is_empty() || secret.is_empty() {
        return Ok(None);
    }
    Ok(Some(AppAuth { token, secret }))
}

async fn setting_or_env(db: &Db, key: &str, env_key: &str) -> Result<String, String> {
    let saved = db.setting(key).await.map_err(|err| err.to_string())?;
    if let Some(saved) = saved.filter(|value| !value.is_empty()) {
        return Ok(saved);
    }
    Ok(std::env::var(env_key).unwrap_or_default())
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

fn channel_for(id: i64, has_app: bool, has_cookie: bool) -> &'static str {
    if !has_app {
        return "cookie";
    }
    if !has_cookie {
        return "app";
    }
    let preferred = if id % 2 == 1 { "app" } else { "cookie" };
    let other = if preferred == "app" { "cookie" } else { "app" };
    if cooling(preferred) && !cooling(other) {
        other
    } else {
        preferred
    }
}

fn channels_cooling(has_app: bool, has_cookie: bool) -> bool {
    (!has_app || cooling("app")) && (!has_cookie || cooling("cookie"))
}

fn cool_store() -> &'static Mutex<(u64, u64)> {
    static COOL: OnceLock<Mutex<(u64, u64)>> = OnceLock::new();
    COOL.get_or_init(|| Mutex::new((0, 0)))
}

fn cooling(channel: &str) -> bool {
    let guard = cool_store().lock().unwrap_or_else(|err| err.into_inner());
    let until = if channel == "app" { guard.0 } else { guard.1 };
    now_secs() < until
}

fn mark_cooling(channel: &str) {
    let mut guard = cool_store().lock().unwrap_or_else(|err| err.into_inner());
    let until = now_secs() + CHANNEL_COOLDOWN;
    if channel == "app" {
        guard.0 = until;
    } else {
        guard.1 = until;
    }
}

fn clear_cooling() {
    *cool_store().lock().unwrap_or_else(|err| err.into_inner()) = (0, 0);
}

fn user_ids() -> &'static Mutex<HashMap<String, String>> {
    static USERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    USERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_user(screen: &str) -> Option<String> {
    user_ids()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(screen)
        .cloned()
}

fn remember_user(screen: &str, id: &str) {
    user_ids()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .insert(screen.to_string(), id.to_string());
}

fn oauth_escape(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn nonce() -> String {
    const ALPH: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("rng");
    buf.iter()
        .map(|byte| ALPH[(*byte as usize) % ALPH.len()] as char)
        .collect()
}

fn oauth1_authorization(
    method: &str,
    url: &str,
    params: &[(&str, &str)],
    token: &str,
    secret: &str,
    nonce: &str,
    timestamp: &str,
) -> String {
    let mut fields = vec![
        (
            "oauth_consumer_key".to_string(),
            APP_CONSUMER_KEY.to_string(),
        ),
        ("oauth_nonce".to_string(), nonce.to_string()),
        (
            "oauth_signature_method".to_string(),
            "HMAC-SHA1".to_string(),
        ),
        ("oauth_timestamp".to_string(), timestamp.to_string()),
        ("oauth_token".to_string(), token.to_string()),
        ("oauth_version".to_string(), "1.0".to_string()),
    ];
    let param_str = {
        let mut signing: Vec<(&str, &str)> = params.to_vec();
        for (key, value) in &fields {
            signing.push((key.as_str(), value.as_str()));
        }
        signing.sort_by(|left, right| left.0.cmp(right.0));
        signing
            .iter()
            .map(|(key, value)| format!("{}={}", oauth_escape(key), oauth_escape(value)))
            .collect::<Vec<_>>()
            .join("&")
    };
    let base = format!(
        "{}&{}&{}",
        method.to_ascii_uppercase(),
        oauth_escape(url),
        oauth_escape(&param_str)
    );
    let key = format!(
        "{}&{}",
        oauth_escape(APP_CONSUMER_SECRET),
        oauth_escape(secret)
    );
    let mut mac = HmacSha1::new_from_slice(key.as_bytes()).expect("hmac key");
    mac.update(base.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    fields.push(("oauth_signature".to_string(), signature));
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    format!(
        "OAuth {}",
        fields
            .iter()
            .map(|(key, value)| format!("{}=\"{}\"", oauth_escape(key), oauth_escape(value)))
            .collect::<Vec<_>>()
            .join(", ")
    )
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
    fn oauth1_header_matches_python_vector() {
        clear_cooling();
        let header = oauth1_authorization(
            "POST",
            "https://api.x.com/graphql/Gb-d6r0vxPOADdG62OEBpQ/UserByScreenName",
            &[
                ("features", FEATURES),
                (
                    "variables",
                    r#"{"screen_name":"Twitter","withSafetyModeUserFields":true}"#,
                ),
            ],
            "token",
            "secret",
            "abcDEF123",
            "1700000000",
        );
        assert!(header.contains("oauth_signature=\"AYXpg%2BcHp0OJZvBMJzEw%2Bjj7kV4%3D\""));
        assert_eq!(channel_for(1, true, true), "app");
        assert_eq!(channel_for(2, true, true), "cookie");
        assert_eq!(channel_for(2, false, true), "cookie");
        assert_eq!(channel_for(2, true, false), "app");
        mark_cooling("cookie");
        assert_eq!(channel_for(2, true, true), "app");
        assert!(channels_cooling(false, true));
        clear_cooling();
        remember_user("unit_test_screen_zzz", "99");
        assert_eq!(cached_user("unit_test_screen_zzz").as_deref(), Some("99"));
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
