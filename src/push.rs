//! 按订阅用户自己的开关发新帖。空的推送渠道表示「已绑定的都发」。

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::Row;

use crate::db::{Db, PushTarget};
use crate::feishu::Note;

pub async fn test_user(db: &Db, user: &crate::db::User, message: &str) -> Result<Vec<serde_json::Value>, &'static str> {
    let text = format!("【测试推送】{}", truncate(message.trim(), 500));
    let picked = channels(&user.push_channels);
    let mut results = Vec::new();
    if picked.wecom && wecom_bound(&user.wecom_webhook) {
        results.push(outcome("wecom", send_wecom_text(&user.wecom_webhook, &text).await));
    }
    if picked.bark && valid_bark_key(&user.bark_key) {
        results.push(outcome("bark", send_bark_text(&user.bark_key, "测试推送", &text).await));
    }
    if picked.telegram {
        let token = telegram_token_for(&user.telegram_bot_token);
        if telegram_token_ok(&token) && chat_id_ok(&user.telegram_chat_id) {
            results.push(outcome("telegram", send_telegram(&token, &user.telegram_chat_id, &text).await));
        }
    }
    if picked.feishu && crate::feishu::configured(db).await.unwrap_or(false) {
        results.push(outcome("feishu", crate::feishu::send_text(db, &text).await));
    }
    if picked.webpush && db.webpush_count(user.id).await.unwrap_or(0) > 0 {
        results.push(outcome("webpush", crate::webpush::send_text(db, user.id, &text).await));
    }
    if results.is_empty() {
        Err("该用户未绑定任何推送渠道")
    } else {
        Ok(results)
    }
}

fn outcome(channel: &str, result: Result<(), String>) -> serde_json::Value {
    match result {
        Ok(()) => serde_json::json!({"channel": channel, "ok": true}),
        Err(err) => serde_json::json!({"channel": channel, "ok": false, "error": err}),
    }
}

pub async fn send_user_text(db: &Db, user_id: i64, text: &str) -> Result<(), String> {
    let Some(user) = db.user_by_id(user_id).await.map_err(|err| err.to_string())? else {
        return Err("用户不存在".into());
    };
    let picked = channels(&user.push_channels);
    let mut sent = false;
    if picked.wecom && wecom_bound(&user.wecom_webhook) {
        send_wecom_text(&user.wecom_webhook, text).await?;
        sent = true;
    }
    if picked.bark && valid_bark_key(&user.bark_key) {
        send_bark_text(&user.bark_key, "每日精选", text).await?;
        sent = true;
    }
    if picked.telegram {
        let token = telegram_token_for(&user.telegram_bot_token);
        if telegram_token_ok(&token) && chat_id_ok(&user.telegram_chat_id) {
            send_telegram(&token, &user.telegram_chat_id, text).await?;
            sent = true;
        }
    }
    if picked.feishu && crate::feishu::configured(db).await.unwrap_or(false) {
        crate::feishu::send_text(db, text).await?;
        sent = true;
    }
    if picked.webpush && db.webpush_count(user_id).await.unwrap_or(0) > 0 {
        crate::webpush::send_text(db, user_id, text).await?;
        sent = true;
    }
    if sent { Ok(()) } else { Err("该用户未绑定任何推送渠道".into()) }
}

pub async fn deliver(db: &Db, kol_id: i64, note: &Note<'_>) {
    let targets = match db.push_targets(kol_id).await {
        Ok(targets) => targets,
        Err(err) => {
            tracing::warn!(kol = kol_id, "读取推送对象失败: {err}");
            return;
        }
    };
    let now = beijing_minutes();
    let mut want_feishu = false;
    for target in &targets {
        if !target.notify_enabled || dnd_blocks(target, now) {
            continue;
        }
        let channels = channels(&target.push_channels);
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|item| item.as_secs() as i64).unwrap_or(0);
        if channels.wecom {
            if let Err(err) = wecom(&target.wecom_webhook, note).await {
                tracing::warn!(kol = kol_id, "企业微信推送失败: {err}");
                note_push_failure(db, &format!("企业微信：{err}")).await;
                let _ = remember_failure(db, kol_id, note.url, "wecom", target.user_id, &err, unix).await;
            }
        }
        if channels.bark {
            if let Err(err) = bark(&target.bark_key, note).await {
                tracing::warn!(kol = kol_id, "Bark 推送失败: {err}");
                note_push_failure(db, &format!("Bark：{err}")).await;
                let _ = remember_failure(db, kol_id, note.url, "bark", target.user_id, &err, unix).await;
            }
        }
        if channels.telegram {
            let token = telegram_token_for(&target.telegram_bot_token);
            if let Err(err) = send_telegram(&token, &target.telegram_chat_id, &plain(note)).await {
                tracing::warn!(kol = kol_id, "Telegram 推送失败: {err}");
                note_push_failure(db, &format!("Telegram：{err}")).await;
                let _ = remember_failure(db, kol_id, note.url, "telegram", target.user_id, &err, unix).await;
            }
        }
        if channels.feishu {
            want_feishu = true;
        }
        if channels.webpush {
            if let Err(err) = crate::webpush::notify_note(db, target.user_id, note, target.favorite).await {
                tracing::warn!(kol = kol_id, "浏览器推送失败: {err}");
                note_push_failure(db, &format!("浏览器：{err}")).await;
                let _ = remember_failure(db, kol_id, note.url, "webpush", target.user_id, &err, unix).await;
            }
        }
    }
    if !want_feishu {
        return;
    }
    if let Err(err) = crate::feishu::notify(
        db,
        Note {
            kol_name: note.kol_name,
            platform: note.platform,
            post_type: note.post_type,
            title: note.title,
            content: note.content,
            url: note.url,
            published_at: note.published_at,
        },
    )
    .await
    {
        tracing::warn!(kol = kol_id, "飞书推送失败: {err}");
        note_push_failure(db, &format!("飞书：{err}")).await;
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|item| item.as_secs() as i64).unwrap_or(0);
        let _ = remember_failure(db, kol_id, note.url, "feishu", 0, &err, unix).await;
    }
}

async fn note_push_failure(db: &Db, detail: &str) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|item| item.as_secs() as i64).unwrap_or(0);
    if let Err(err) = crate::alerts::push_failure(db, now, detail, |message| async move { crate::alerts::notify_admins(db, &message).await }).await {
        tracing::warn!("推送失败告警失败: {err}");
    }
}

const RETRY_DELAYS: [i64; 3] = [60, 300, 900];

pub async fn remember_failure(db: &Db, kol_id: i64, url: &str, channel: &str, user_id: i64, error: &str, now: i64) -> Result<(), sqlx::Error> {
    let row = sqlx::query("SELECT id, platform, external_id, url FROM posts WHERE kol_id = ? ORDER BY id DESC LIMIT 1")
        .bind(kol_id)
        .fetch_optional(db.pool())
        .await?;
    let Some(row) = row else { return Ok(()) };
    let stored: String = row.get("url");
    if !url.is_empty() && stored != url {
        return Ok(());
    }
    let post_id: i64 = row.get("id");
    let platform: String = row.get("platform");
    let external_id: String = row.get("external_id");
    sqlx::query(
        "INSERT INTO push_retries (channel, user_id, platform, external_id, post_id, attempts, next_at)
         VALUES (?, ?, ?, ?, ?, 0, ?)
         ON CONFLICT(channel, user_id, platform, external_id) DO NOTHING",
    )
    .bind(channel)
    .bind(user_id)
    .bind(&platform)
    .bind(&external_id)
    .bind(post_id)
    .bind(now + RETRY_DELAYS[0])
    .execute(db.pool())
    .await?;
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_logs WHERE post_id = ? AND channel = ? AND user_id = ? AND status = 'failed'")
        .bind(post_id)
        .bind(channel)
        .bind(user_id)
        .fetch_one(db.pool())
        .await?;
    if open == 0 {
        db.add_push_log(post_id, channel, "failed", error, Some(user_id)).await?;
    }
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_retries").fetch_one(db.pool()).await?;
    db.set_setting("stats_retry_pending", &pending.to_string()).await?;
    Ok(())
}

pub async fn retry_due<F, Fut>(db: &Db, now: i64, mut send: F) -> Result<usize, sqlx::Error>
where
    F: FnMut(i64, String, i64) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let rows = sqlx::query(
        "SELECT channel, user_id, platform, external_id, post_id, attempts FROM push_retries WHERE next_at <= ? ORDER BY next_at, post_id",
    )
    .bind(now)
    .fetch_all(db.pool())
    .await?;
    let mut done = 0;
    for row in rows {
        let channel: String = row.get("channel");
        let user_id: i64 = row.get("user_id");
        let platform: String = row.get("platform");
        let external_id: String = row.get("external_id");
        let post_id: i64 = row.get("post_id");
        let attempts: i64 = row.get("attempts");
        if channel != "feishu" {
            let quiet = sqlx::query(
                "SELECT u.dnd_start, u.dnd_end, u.dnd_allow_favorite, COALESCE(s.favorite, 0) AS favorite
                 FROM users u JOIN posts p ON p.id = ?
                 LEFT JOIN subscriptions s ON s.user_id = u.id AND s.kol_id = p.kol_id
                 WHERE u.id = ?",
            )
            .bind(post_id)
            .bind(user_id)
            .fetch_optional(db.pool())
            .await?;
            if let Some(quiet) = quiet {
                let target = PushTarget {
                    notify_enabled: true,
                    push_channels: String::new(),
                    dnd_start: quiet.get("dnd_start"),
                    dnd_end: quiet.get("dnd_end"),
                    dnd_allow_favorite: quiet.get::<i64, _>("dnd_allow_favorite") != 0,
                    favorite: quiet.get::<i64, _>("favorite") != 0,
                    wecom_webhook: String::new(),
                    bark_key: String::new(),
                    user_id,
                    telegram_chat_id: String::new(),
                    telegram_bot_token: String::new(),
                };
                if dnd_blocks(&target, beijing_minutes()) {
                    sqlx::query("UPDATE push_retries SET next_at = ? WHERE channel = ? AND user_id = ? AND platform = ? AND external_id = ?")
                        .bind(now + 60)
                        .bind(&channel)
                        .bind(user_id)
                        .bind(&platform)
                        .bind(&external_id)
                        .execute(db.pool())
                        .await?;
                    continue;
                }
            }
        }
        let subscribed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM subscriptions s JOIN users u ON u.id = s.user_id JOIN posts p ON p.kol_id = s.kol_id
             WHERE s.user_id = ? AND p.id = ? AND u.notify_enabled = 1",
        )
        .bind(user_id)
        .bind(post_id)
        .fetch_one(db.pool())
        .await?;
        if channel != "feishu" && subscribed == 0 {
            sqlx::query("DELETE FROM push_retries WHERE channel = ? AND user_id = ? AND platform = ? AND external_id = ?")
                .bind(&channel)
                .bind(user_id)
                .bind(&platform)
                .bind(&external_id)
                .execute(db.pool())
                .await?;
            continue;
        }
        match send(post_id, channel.clone(), user_id).await {
            Ok(()) => {
                sqlx::query("DELETE FROM push_retries WHERE channel = ? AND user_id = ? AND platform = ? AND external_id = ?")
                    .bind(&channel)
                    .bind(user_id)
                    .bind(&platform)
                    .bind(&external_id)
                    .execute(db.pool())
                    .await?;
                sqlx::query("UPDATE push_logs SET status = 'success', error = '' WHERE id = (SELECT id FROM push_logs WHERE post_id = ? AND channel = ? AND user_id = ? AND status = 'failed' ORDER BY id DESC LIMIT 1)")
                    .bind(post_id)
                    .bind(&channel)
                    .bind(user_id)
                    .execute(db.pool())
                    .await?;
                done += 1;
            }
            Err(_) => {
                let next = attempts + 1;
                if next >= RETRY_DELAYS.len() as i64 {
                    sqlx::query("DELETE FROM push_retries WHERE channel = ? AND user_id = ? AND platform = ? AND external_id = ?")
                        .bind(&channel)
                        .bind(user_id)
                        .bind(&platform)
                        .bind(&external_id)
                        .execute(db.pool())
                        .await?;
                } else {
                    sqlx::query("UPDATE push_retries SET attempts = ?, next_at = ? WHERE channel = ? AND user_id = ? AND platform = ? AND external_id = ?")
                        .bind(next)
                        .bind(now + RETRY_DELAYS[next as usize])
                        .bind(&channel)
                        .bind(user_id)
                        .bind(&platform)
                        .bind(&external_id)
                        .execute(db.pool())
                        .await?;
                }
            }
        }
    }
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_retries").fetch_one(db.pool()).await?;
    db.set_setting("stats_retry_pending", &pending.to_string()).await?;
    Ok(done)
}

pub async fn retry_due_live(db: &Db, now: i64) -> Result<usize, sqlx::Error> {
    retry_due(db, now, |post_id, channel, user_id| async move {
        let row = sqlx::query("SELECT k.name, p.platform, p.post_type, p.title, p.content, p.url, p.published_at FROM posts p JOIN kols k ON k.id = p.kol_id WHERE p.id = ?")
            .bind(post_id)
            .fetch_optional(db.pool())
            .await
            .map_err(|err| err.to_string())?;
        let Some(row) = row else { return Err("帖子不存在".into()) };
        let name: String = row.get("name");
        let platform: String = row.get("platform");
        let post_type: String = row.get("post_type");
        let title: String = row.get("title");
        let content: String = row.get("content");
        let url: String = row.get("url");
        let published_at: String = row.get("published_at");
        let text = {
            let note = Note {
                kol_name: &name,
                platform: &platform,
                post_type: &post_type,
                title: &title,
                content: &content,
                url: &url,
                published_at: &published_at,
            };
            plain(&note)
        };
        match channel.as_str() {
            "wecom" => {
                let url: String = sqlx::query_scalar("SELECT wecom_webhook FROM users WHERE id = ?").bind(user_id).fetch_one(db.pool()).await.map_err(|err| err.to_string())?;
                send_wecom_text(&url, &text).await
            }
            "bark" => {
                let key: String = sqlx::query_scalar("SELECT bark_key FROM users WHERE id = ?").bind(user_id).fetch_one(db.pool()).await.map_err(|err| err.to_string())?;
                send_bark_text(&key, "订阅更新", &text).await
            }
            "telegram" => {
                let row = sqlx::query("SELECT telegram_bot_token, telegram_chat_id FROM users WHERE id = ?").bind(user_id).fetch_one(db.pool()).await.map_err(|err| err.to_string())?;
                let token = telegram_token_for(&row.get::<String, _>("telegram_bot_token"));
                send_telegram(&token, &row.get::<String, _>("telegram_chat_id"), &text).await
            }
            "webpush" => crate::webpush::send_text(db, user_id, &text).await,
            "feishu" => crate::feishu::send_text(db, &text).await,
            _ => Err("未知渠道".into()),
        }
    })
    .await
}

struct Channels {
    wecom: bool,
    bark: bool,
    feishu: bool,
    telegram: bool,
    webpush: bool,
}

fn channels(raw: &str) -> Channels {
    let picked: Vec<&str> = raw.split(',').map(str::trim).filter(|item| !item.is_empty()).collect();
    if picked.is_empty() {
        return Channels { wecom: true, bark: true, feishu: true, telegram: true, webpush: true };
    }
    Channels {
        wecom: picked.contains(&"wecom"),
        bark: picked.contains(&"bark"),
        feishu: picked.contains(&"feishu"),
        telegram: picked.contains(&"telegram"),
        webpush: picked.contains(&"webpush"),
    }
}

fn dnd_blocks(target: &PushTarget, now: u32) -> bool {
    if target.favorite && target.dnd_allow_favorite {
        return false;
    }
    let (Some(start), Some(end)) = (clock(&target.dnd_start), clock(&target.dnd_end)) else {
        return false;
    };
    if start == end {
        return false;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

fn clock(value: &str) -> Option<u32> {
    let (hour, minute) = value.split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

fn beijing_minutes() -> u32 {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    ((secs + 8 * 3600) % 86400 / 60) as u32
}

async fn wecom(url: &str, note: &Note<'_>) -> Result<(), String> {
    send_wecom_text(url, &plain(note)).await
}

async fn send_wecom_text(url: &str, text: &str) -> Result<(), String> {
    if !wecom_bound(url) {
        return Ok(());
    }
    let body = serde_json::json!({"msgtype": "text", "text": {"content": text}}).to_string();
    let url = url.to_string();
    tokio::task::spawn_blocking(move || post_json(&url, &body))
        .await
        .map_err(|err| err.to_string())?
}

fn wecom_bound(url: &str) -> bool {
    url.starts_with("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=")
}

async fn bark(key: &str, note: &Note<'_>) -> Result<(), String> {
    let title = format!("{} · {}", note.kol_name, platform_label(note.platform));
    send_bark_text(key, &title, &truncate(&plain(note), 500)).await
}

async fn send_bark_text(key: &str, title: &str, body: &str) -> Result<(), String> {
    if !valid_bark_key(key) {
        return Ok(());
    }
    let url = format!("https://api.day.app/{}/{}/{}", key, url_encode(title), url_encode(body));
    tokio::task::spawn_blocking(move || post_json(&url, ""))
        .await
        .map_err(|err| err.to_string())?
}

fn telegram_token_for(custom: &str) -> String {
    if telegram_token_ok(custom) {
        custom.to_string()
    } else {
        std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default()
    }
}

pub fn telegram_token_ok(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else {
        return false;
    };
    (6..=16).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && (20..=128).contains(&secret.len())
        && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn chat_id_ok(chat_id: &str) -> bool {
    let digits = chat_id.strip_prefix('-').unwrap_or(chat_id);
    (1..=20).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
}

pub fn parse_telegram_bind(me_body: &str, updates_body: &str) -> Result<(String, String), String> {
    let me: serde_json::Value = serde_json::from_str(me_body).map_err(|_| "token 无效：未知错误".to_string())?;
    if me.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let desc = me.get("description").and_then(|v| v.as_str()).unwrap_or("未知错误");
        return Err(format!("token 无效：{desc}"));
    }
    let username = me["result"]["username"].as_str().unwrap_or("").to_string();
    let updates: serde_json::Value = serde_json::from_str(updates_body).map_err(|_| "获取会话失败：未知错误".to_string())?;
    if updates.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let desc = updates.get("description").and_then(|v| v.as_str()).unwrap_or("未知错误");
        return Err(format!("获取会话失败：{desc}"));
    }
    let mut chat_id = String::new();
    if let Some(items) = updates["result"].as_array() {
        for update in items {
            let id = &update["message"]["chat"]["id"];
            if let Some(id) = id.as_i64() {
                chat_id = id.to_string();
            } else if let Some(id) = id.as_str().filter(|value| chat_id_ok(value)) {
                chat_id = id.to_string();
            }
        }
    }
    if chat_id.is_empty() {
        return Err("请先给你的机器人发一条消息（如 /start），再点保存".into());
    }
    Ok((username, chat_id))
}

pub async fn resolve_telegram_bot(token: &str) -> Result<(String, String), String> {
    let token = token.trim().to_string();
    if !telegram_token_ok(&token) {
        return Err("token 无效".into());
    }
    let token_for_call = token.clone();
    let (me, updates) = tokio::task::spawn_blocking(move || {
        let me = telegram_get(&token_for_call, "getMe")?;
        let updates = telegram_get(&token_for_call, "getUpdates")?;
        Ok::<_, String>((me, updates))
    })
    .await
    .map_err(|err| redact_token(format!("无法连接 Telegram：{err}"), &token))?
    .map_err(|err| redact_token(format!("无法连接 Telegram：{err}"), &token))?;
    parse_telegram_bind(&me, &updates)
}

async fn send_telegram(token: &str, chat_id: &str, text: &str) -> Result<(), String> {
    if !telegram_token_ok(token) || !chat_id_ok(chat_id) {
        return Ok(());
    }
    let body = serde_json::json!({
        "chat_id": chat_id,
        "text": truncate(text, 4000),
        "disable_web_page_preview": true,
    })
    .to_string();
    let token = token.to_string();
    tokio::task::spawn_blocking(move || {
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        post_json(&url, &body).map_err(|err| redact_token(err, &token))
    })
    .await
    .map_err(|err| err.to_string())?
}

fn telegram_get(token: &str, method: &str) -> Result<String, String> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .build();
    let response = match agent.get(&url).call() {
        Ok(resp) => resp.into_string().map_err(|err| err.to_string()),
        Err(ureq::Error::Status(_, resp)) => resp.into_string().map_err(|err| err.to_string()),
        Err(err) => Err(err.to_string()),
    };
    response.map_err(|err| redact_token(err, token))
}

fn redact_token(err: String, token: &str) -> String {
    if token.is_empty() { err } else { err.replace(token, "<redacted>") }
}

fn valid_bark_key(key: &str) -> bool {
    let len = key.len();
    (10..=100).contains(&len)
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn plain(note: &Note<'_>) -> String {
    let kind = if note.post_type == "reply" { " · 回复" } else { "" };
    let body = if note.content.is_empty() { note.title } else { note.content };
    let body = if body.is_empty() { "（无正文）" } else { body };
    let mut text = format!("{} · {}{kind}\n\n{}", note.kol_name, platform_label(note.platform), truncate(body, 1600));
    if !note.published_at.is_empty() {
        text.push_str("\n🕐 ");
        text.push_str(note.published_at);
    }
    if note.url.starts_with("http://") || note.url.starts_with("https://") {
        text.push_str("\n🔗 ");
        text.push_str(note.url);
    }
    text
}

pub(crate) fn platform_label(platform: &str) -> &str {
    match platform {
        "xueqiu" => "雪球",
        "combination" => "雪球组合",
        "weibo" => "微博",
        "twitter" => "X",
        "ima" => "IMA",
        "zsxq" => "知识星球",
        "truth" => "Truth",
        other => other,
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn post_json(url: &str, body: &str) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .build();
    let request = agent.post(url);
    let response = if body.is_empty() {
        request.call()
    } else {
        request.set("Content-Type", "application/json").send_string(body)
    };
    match response {
        Ok(resp) if (200..300).contains(&resp.status()) => Ok(()),
        Ok(resp) => Err(format!("推送 HTTP {}", resp.status())),
        Err(ureq::Error::Status(code, _)) => Err(format!("推送 HTTP {code}")),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(start: &str, end: &str, favorite: bool, allow: bool) -> PushTarget {
        PushTarget {
            notify_enabled: true,
            push_channels: String::new(),
            dnd_start: start.into(),
            dnd_end: end.into(),
            dnd_allow_favorite: allow,
            favorite,
            wecom_webhook: String::new(),
            bark_key: String::new(),
            user_id: 0,
            telegram_chat_id: String::new(),
            telegram_bot_token: String::new(),
        }
    }

    #[test]
    fn quiet_hours_skip_ordinary_posts() {
        let quiet = target("22:00", "08:00", false, false);
        assert!(dnd_blocks(&quiet, 23 * 60));
        assert!(dnd_blocks(&quiet, 7 * 60));
        assert!(!dnd_blocks(&quiet, 12 * 60));
        assert!(!dnd_blocks(&target("22:00", "08:00", true, true), 23 * 60));
        assert!(!dnd_blocks(&target("", "", false, false), 23 * 60));
    }

    #[test]
    fn empty_channel_list_means_all() {
        let all = channels("");
        assert!(all.feishu && all.wecom && all.bark && all.telegram);
        let only = channels("wecom, bark");
        assert!(only.wecom && only.bark && !only.feishu && !only.telegram);
        assert!(wecom_bound("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abc"));
        assert!(!wecom_bound("https://evil.example/hook"));
    }

    #[tokio::test]
    async fn unbound_user_is_not_sent() {
        let path = std::env::temp_dir().join(format!(
            "vpush-push-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let mut user = db.user_by_username("admin").await.unwrap().unwrap();
        user.push_channels = "wecom".into();
        let err = test_user(&db, &user, "你好").await.unwrap_err();
        assert_eq!(err, "该用户未绑定任何推送渠道");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn telegram_bind_uses_the_latest_chat_and_rejects_a_silent_bot() {
        let me = r#"{"ok":true,"result":{"username":"dav_bot"}}"#;
        let updates = r#"{"ok":true,"result":[{"message":{"chat":{"id":111}}},{"message":{"chat":{"id":-222}}}]}"#;
        assert_eq!(parse_telegram_bind(me, updates).unwrap(), ("dav_bot".into(), "-222".into()));
        let silent = r#"{"ok":true,"result":[]}"#;
        assert!(parse_telegram_bind(me, silent).unwrap_err().contains("先给你的机器人发一条消息"));
        assert!(parse_telegram_bind(r#"{"ok":false,"description":"Unauthorized"}"#, silent).unwrap_err().contains("Unauthorized"));
        assert!(telegram_token_ok("123456:ABCDEFGHIJKLMNOPQRST"));
        assert!(!telegram_token_ok("123456:short"));
        assert!(!telegram_token_ok("bad/token:ABCDEFGHIJKLMNOPQRST"));
        assert_eq!(redact_token("https://api.telegram.org/bot123456:ABCDEFGHIJKLMNOPQRST/x".into(), "123456:ABCDEFGHIJKLMNOPQRST"), "https://api.telegram.org/bot<redacted>/x");
    }

    #[tokio::test]
    async fn failed_push_retries_then_stops() {
        let path = std::env::temp_dir().join(format!(
            "vpush-retry-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, notify_enabled) VALUES ('甲', 'x', 1)").execute(db.pool()).await.unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = '甲'").fetch_one(db.pool()).await.unwrap();
        let kol = db.add_kol("xueqiu", "段永平", "111", None, false, false, false).await.unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id, type) VALUES (?, ?, 'post')").bind(user).bind(kol).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'p1', '标题', 'https://xueqiu.com/p/1')").bind(kol).execute(db.pool()).await.unwrap();
        remember_failure(&db, kol, "https://xueqiu.com/p/1", "wecom", user, "超时", 1_000).await.unwrap();
        remember_failure(&db, kol, "https://other", "wecom", user, "不应入队", 1_000).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries").fetch_one(db.pool()).await.unwrap(), 1);
        assert_eq!(retry_due(&db, 1_000, |_, _, _| async { Ok(()) }).await.unwrap(), 0);
        assert_eq!(retry_due(&db, 1_060, |_, _, _| async { Err("仍失败".into()) }).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT attempts FROM push_retries").fetch_one(db.pool()).await.unwrap(), 1);
        assert_eq!(retry_due(&db, 1_060 + 300, |_, channel, _| async move {
            assert_eq!(channel, "wecom");
            Ok(())
        }).await.unwrap(), 1);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries").fetch_one(db.pool()).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM push_logs").fetch_one(db.pool()).await.unwrap(), "success");
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'p2', '二', 'https://xueqiu.com/p/2')").bind(kol).execute(db.pool()).await.unwrap();
        remember_failure(&db, kol, "https://xueqiu.com/p/2", "bark", user, "超时", 2_000).await.unwrap();
        for delay in [60, 300, 900] {
            let at: i64 = sqlx::query_scalar("SELECT next_at FROM push_retries").fetch_one(db.pool()).await.unwrap();
            assert_eq!(retry_due(&db, at, |_, _, _| async { Err("失败".into()) }).await.unwrap(), 0);
            let _ = delay;
        }
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries").fetch_one(db.pool()).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM push_logs WHERE channel = 'bark'").fetch_one(db.pool()).await.unwrap(), "failed");
        sqlx::query("DELETE FROM subscriptions").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'p3', '三', 'https://xueqiu.com/p/3')").bind(kol).execute(db.pool()).await.unwrap();
        remember_failure(&db, kol, "https://xueqiu.com/p/3", "telegram", user, "超时", 3_000).await.unwrap();
        assert_eq!(retry_due(&db, 3_060, |_, _, _| async { Ok(()) }).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries").fetch_one(db.pool()).await.unwrap(), 0);
        assert_eq!(db.setting("stats_retry_pending").await.unwrap().as_deref(), Some("0"));
        let _ = std::fs::remove_file(&path);
    }
}
