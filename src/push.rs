//! 按订阅用户自己的开关发新帖。空的推送渠道表示「已绑定的都发」。

use std::collections::VecDeque;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sqlx::Row;

use crate::db::{Db, PushTarget};
use crate::feishu::Note;

pub async fn test_user(
    db: &Db,
    user: &crate::db::User,
    message: &str,
) -> Result<Vec<serde_json::Value>, &'static str> {
    let text = format!("【测试推送】{}", truncate(message.trim(), 500));
    let picked = channels(&user.push_channels);
    let mut results = Vec::new();
    if picked.wecom && wecom_bound(&user.wecom_webhook) {
        results.push(outcome(
            "wecom",
            send_wecom_text(&user.wecom_webhook, &text).await,
        ));
    }
    if picked.bark && valid_bark_key(&user.bark_key) {
        results.push(outcome(
            "bark",
            send_bark_text(&user.bark_key, "测试推送", &text).await,
        ));
    }
    if picked.telegram && !user.telegram_chat_id.trim().is_empty() {
        let key = crate::feishu_personal::credential_key().unwrap_or_default();
        match telegram_secret(&user.telegram_bot_token, &key) {
            Ok(token) => results.push(outcome(
                "telegram",
                send_telegram(&token, &user.telegram_chat_id, &text).await,
            )),
            Err(err) => results.push(outcome("telegram", Err(err))),
        }
    }
    if picked.feishu {
        results.push(outcome(
            "feishu",
            crate::feishu::deliver_text(db, user.id, &text).await,
        ));
    }
    if picked.webpush && db.webpush_count(user.id).await.unwrap_or(0) > 0 {
        results.push(outcome(
            "webpush",
            crate::webpush::send_text(db, user.id, &text).await,
        ));
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
    let Some(user) = db
        .user_by_id(user_id)
        .await
        .map_err(|err| err.to_string())?
    else {
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
    if picked.telegram && !user.telegram_chat_id.trim().is_empty() {
        let key = crate::feishu_personal::credential_key().unwrap_or_default();
        let token = telegram_secret(&user.telegram_bot_token, &key)?;
        send_telegram(&token, &user.telegram_chat_id, text).await?;
        sent = true;
    }
    if picked.feishu {
        match crate::feishu::deliver_text(db, user_id, text).await {
            Ok(()) => sent = true,
            Err(err) if err == "飞书未绑定" => {}
            Err(err) => return Err(err),
        }
    }
    if picked.webpush && db.webpush_count(user_id).await.unwrap_or(0) > 0 {
        crate::webpush::send_text(db, user_id, text).await?;
        sent = true;
    }
    if sent {
        Ok(())
    } else {
        Err("该用户未绑定任何推送渠道".into())
    }
}

pub async fn deliver(db: &Db, kol_id: i64, note: &Note<'_>) {
    let targets = match db.push_targets(kol_id).await {
        Ok(targets) => targets,
        Err(err) => {
            tracing::warn!(kol = kol_id, "读取推送对象失败: {err}");
            return;
        }
    };
    let rich_messages = telegram_rich_messages(db).await;
    let now = beijing_minutes();
    let telegram_post = load_telegram_identity(db, kol_id, note.platform, note.external_id).await;
    for target in &targets {
        if !target.notify_enabled || dnd_blocks(target, now) {
            continue;
        }
        let channels = channels(&target.push_channels);
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|item| item.as_secs() as i64)
            .unwrap_or(0);
        if channels.wecom && wecom_bound(&target.wecom_webhook) {
            if let Err(err) = wecom(&target.wecom_webhook, note).await {
                tracing::warn!(kol = kol_id, "企业微信推送失败: {err}");
                note_push_failure(db, &format!("企业微信：{err}")).await;
                remember_failure_logged(
                    db,
                    kol_id,
                    (note.platform, note.external_id),
                    "wecom",
                    target.user_id,
                    &err,
                    unix,
                )
                .await;
            } else {
                remember_success(
                    db,
                    kol_id,
                    (note.platform, note.external_id),
                    "wecom",
                    target.user_id,
                )
                .await;
            }
        }
        if channels.bark && valid_bark_key(&target.bark_key) {
            if let Err(err) = bark(&target.bark_key, note).await {
                tracing::warn!(kol = kol_id, "Bark 推送失败: {err}");
                note_push_failure(db, &format!("Bark：{err}")).await;
                remember_failure_logged(
                    db,
                    kol_id,
                    (note.platform, note.external_id),
                    "bark",
                    target.user_id,
                    &err,
                    unix,
                )
                .await;
            } else {
                remember_success(
                    db,
                    kol_id,
                    (note.platform, note.external_id),
                    "bark",
                    target.user_id,
                )
                .await;
            }
        }
        if channels.telegram && !target.telegram_chat_id.trim().is_empty() {
            let key = crate::feishu_personal::credential_key().unwrap_or_default();
            match telegram_secret(&target.telegram_bot_token, &key) {
                Ok(token) => {
                    let result = match &telegram_post {
                        Ok(Some(post)) => {
                            send_telegram_post(
                                &token,
                                &target.telegram_chat_id,
                                Some(post),
                                target.user_id,
                                db,
                                rich_messages,
                            )
                            .await
                        }
                        Ok(None) => Err("帖子不存在".into()),
                        Err(err) => Err(format!("读取帖子失败: {err}")),
                    };
                    if let Err(err) = result {
                        tracing::warn!(kol = kol_id, "Telegram 推送失败: {err}");
                        note_push_failure(db, &format!("Telegram：{err}")).await;
                        remember_failure_logged(
                            db,
                            kol_id,
                            (note.platform, note.external_id),
                            "telegram",
                            target.user_id,
                            &err,
                            unix,
                        )
                        .await;
                    } else {
                        remember_success(
                            db,
                            kol_id,
                            (note.platform, note.external_id),
                            "telegram",
                            target.user_id,
                        )
                        .await;
                    }
                }
                Err(err) => {
                    tracing::warn!(kol = kol_id, "Telegram 推送失败: {err}");
                    note_push_failure(db, &format!("Telegram：{err}")).await;
                    remember_failure_logged(
                        db,
                        kol_id,
                        (note.platform, note.external_id),
                        "telegram",
                        target.user_id,
                        &err,
                        unix,
                    )
                    .await;
                }
            }
        }
        if channels.feishu {
            let empty = serde_json::Value::Null;
            let (category, tags, detail, favorite, keyword) = match &telegram_post {
                Ok(Some(post)) => {
                    let (favorite, keyword) = telegram_reasons(db, post, target.user_id)
                        .await
                        .unwrap_or((target.favorite, false));
                    (
                        post.category.as_str(),
                        &post.tags,
                        &post.detail,
                        favorite,
                        keyword,
                    )
                }
                _ => ("", &empty, &empty, target.favorite, false),
            };
            let ctx = crate::feishu::CardContext {
                category,
                tags,
                detail,
                favorite,
                keyword,
            };
            match crate::feishu::deliver_user(db, target.user_id, note, &ctx).await {
                Ok(()) => {
                    remember_success(
                        db,
                        kol_id,
                        (note.platform, note.external_id),
                        "feishu",
                        target.user_id,
                    )
                    .await;
                }
                Err(err) if err == "飞书未绑定" => {}
                Err(err) => {
                    tracing::warn!(kol = kol_id, user = target.user_id, "飞书推送失败: {err}");
                    note_push_failure(db, &format!("飞书：{err}")).await;
                    remember_failure_logged(
                        db,
                        kol_id,
                        (note.platform, note.external_id),
                        "feishu",
                        target.user_id,
                        &err,
                        unix,
                    )
                    .await;
                }
            }
        }
        if channels.webpush {
            if let Err(err) =
                crate::webpush::notify_note(db, target.user_id, note, target.favorite).await
            {
                tracing::warn!(kol = kol_id, "浏览器推送失败: {err}");
                note_push_failure(db, &format!("浏览器：{err}")).await;
                remember_failure_logged(
                    db,
                    kol_id,
                    (note.platform, note.external_id),
                    "webpush",
                    target.user_id,
                    &err,
                    unix,
                )
                .await;
            } else {
                remember_success(
                    db,
                    kol_id,
                    (note.platform, note.external_id),
                    "webpush",
                    target.user_id,
                )
                .await;
            }
        }
    }
    if let Err(err) = crate::feishu::notify(
        db,
        Note {
            kol_name: note.kol_name,
            platform: note.platform,
            external_id: note.external_id,
            post_type: note.post_type,
            title: note.title,
            content: note.content,
            url: note.url,
            published_at: note.published_at,
        },
    )
    .await
    {
        tracing::warn!(kol = kol_id, "飞书群机器人失败: {err}");
    }
}

async fn note_push_failure(db: &Db, detail: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|item| item.as_secs() as i64)
        .unwrap_or(0);
    if let Err(err) = crate::alerts::push_failure(db, now, detail, |message| async move {
        crate::alerts::notify_admins(db, &message).await
    })
    .await
    {
        tracing::warn!("推送失败告警失败: {err}");
    }
}

const RETRY_DELAYS: [i64; 3] = [60, 300, 900];

async fn remember_failure_logged(
    db: &Db,
    kol_id: i64,
    identity: (&str, &str),
    channel: &str,
    user_id: i64,
    error: &str,
    now: i64,
) {
    if let Err(err) = remember_failure(db, kol_id, identity, channel, user_id, error, now).await {
        tracing::warn!(kol = kol_id, channel, "记录推送失败以便重试失败: {err}");
    }
}

async fn remember_success(
    db: &Db,
    kol_id: i64,
    identity: (&str, &str),
    channel: &str,
    user_id: i64,
) {
    let row =
        sqlx::query("SELECT id FROM posts WHERE platform = ? AND external_id = ? AND kol_id = ?")
            .bind(identity.0)
            .bind(identity.1)
            .bind(kol_id)
            .fetch_optional(db.pool())
            .await;
    let row = match row {
        Ok(row) => row,
        Err(err) => {
            tracing::warn!(kol = kol_id, channel, "记录推送成功失败: {err}");
            return;
        }
    };
    let Some(row) = row else {
        return;
    };
    if let Err(err) = db
        .add_push_log(row.get("id"), channel, "success", "", Some(user_id))
        .await
    {
        tracing::warn!(kol = kol_id, channel, "记录推送成功失败: {err}");
    }
}

pub async fn remember_failure(
    db: &Db,
    kol_id: i64,
    identity: (&str, &str),
    channel: &str,
    user_id: i64,
    error: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    let row =
        sqlx::query("SELECT id FROM posts WHERE platform = ? AND external_id = ? AND kol_id = ?")
            .bind(identity.0)
            .bind(identity.1)
            .bind(kol_id)
            .fetch_optional(db.pool())
            .await?;
    let Some(row) = row else {
        tracing::warn!(
            kol = kol_id,
            platform = identity.0,
            external_id = identity.1,
            channel,
            "未找到推送失败对应的帖子，跳过重试入队"
        );
        return Ok(());
    };
    let post_id: i64 = row.get("id");
    let platform = identity.0;
    let external_id = identity.1;
    sqlx::query(
        "INSERT INTO push_retries (channel, user_id, platform, external_id, post_id, attempts, next_at)
         VALUES (?, ?, ?, ?, ?, 0, ?)
         ON CONFLICT(channel, user_id, platform, external_id) DO NOTHING",
    )
    .bind(channel)
    .bind(user_id)
    .bind(platform)
    .bind(external_id)
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
        db.add_push_log(post_id, channel, "failed", error, Some(user_id))
            .await?;
    }
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_retries")
        .fetch_one(db.pool())
        .await?;
    db.set_setting("stats_retry_pending", &pending.to_string())
        .await?;
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
        let subscribed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM subscriptions s JOIN users u ON u.id = s.user_id JOIN posts p ON p.kol_id = s.kol_id
             WHERE s.user_id = ? AND p.id = ? AND u.notify_enabled = 1",
        )
        .bind(user_id)
        .bind(post_id)
        .fetch_one(db.pool())
        .await?;
        if subscribed == 0 {
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
                let mut tx = db.pool().begin().await?;
                sqlx::query("DELETE FROM push_retries WHERE channel = ? AND user_id = ? AND platform = ? AND external_id = ?")
                    .bind(&channel)
                    .bind(user_id)
                    .bind(&platform)
                    .bind(&external_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE push_logs SET status = 'success', error = '' WHERE id = (SELECT id FROM push_logs WHERE post_id = ? AND channel = ? AND user_id = ? AND status = 'failed' ORDER BY id DESC LIMIT 1)")
                    .bind(post_id)
                    .bind(&channel)
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
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
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_retries")
        .fetch_one(db.pool())
        .await?;
    db.set_setting("stats_retry_pending", &pending.to_string())
        .await?;
    Ok(done)
}

pub async fn retry_due_live(db: &Db, now: i64) -> Result<usize, sqlx::Error> {
    let rich_messages = telegram_rich_messages(db).await;
    retry_due(db, now, |post_id, channel, user_id| async move {
        let row = sqlx::query("SELECT k.name, p.platform, p.external_id, p.post_type, p.title, p.content, p.url, p.published_at FROM posts p JOIN kols k ON k.id = p.kol_id WHERE p.id = ?")
            .bind(post_id)
            .fetch_optional(db.pool())
            .await
            .map_err(|err| err.to_string())?;
        let Some(row) = row else { return Err("帖子不存在".into()) };
        let name: String = row.get("name");
        let platform: String = row.get("platform");
        let external_id: String = row.get("external_id");
        let post_type: String = row.get("post_type");
        let title: String = row.get("title");
        let content: String = row.get("content");
        let url: String = row.get("url");
        let published_at: String = row.get("published_at");
        let text = {
            let note = Note {
                kol_name: &name,
                platform: &platform,
                external_id: &external_id,
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
                let key = crate::feishu_personal::credential_key().unwrap_or_default();
                let token = telegram_secret(&row.get::<String, _>("telegram_bot_token"), &key)?;
                let post = load_telegram_post(db, post_id).await.map_err(|err| err.to_string())?
                    .ok_or_else(|| "帖子不存在".to_string())?;
                send_telegram_post(
                    &token,
                    &row.get::<String, _>("telegram_chat_id"),
                    Some(&post),
                    user_id,
                    db,
                    rich_messages,
                )
                .await
            }
            "webpush" => crate::webpush::send_text(db, user_id, &text).await,
            "feishu" => {
                let post = load_telegram_post(db, post_id)
                    .await
                    .map_err(|err| err.to_string())?
                    .ok_or_else(|| "帖子不存在".to_string())?;
                let (favorite, keyword) =
                    telegram_reasons(db, &post, user_id).await.unwrap_or((false, false));
                let note = Note {
                    kol_name: &post.kol_name,
                    platform: &post.platform,
                    external_id: &external_id,
                    post_type: &post.post_type,
                    title: &post.title,
                    content: &post.content,
                    url: &post.url,
                    published_at: &post.published_at,
                };
                let ctx = crate::feishu::CardContext {
                    category: &post.category,
                    tags: &post.tags,
                    detail: &post.detail,
                    favorite,
                    keyword,
                };
                crate::feishu::deliver_user(db, user_id, &note, &ctx).await
            }
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
    let picked: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect();
    if picked.is_empty() {
        return Channels {
            wecom: true,
            bark: true,
            feishu: true,
            telegram: true,
            webpush: true,
        };
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
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ((secs + 8 * 3600) % 86400 / 60) as u32
}

async fn wecom(url: &str, note: &Note<'_>) -> Result<(), String> {
    send_wecom_text(url, &plain(note)).await
}

async fn send_wecom_text(url: &str, text: &str) -> Result<(), String> {
    if !wecom_bound(url) {
        return Err("企业微信未绑定".into());
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
        return Err("Bark 未绑定".into());
    }
    let url = format!(
        "https://api.day.app/{}/{}/{}",
        key,
        url_encode(title),
        url_encode(body)
    );
    tokio::task::spawn_blocking(move || post_json(&url, ""))
        .await
        .map_err(|err| err.to_string())?
}

pub fn telegram_secret(stored: &str, key: &str) -> Result<String, String> {
    let token = if let Some(ciphertext) = stored.strip_prefix("enc2:") {
        if key.is_empty() {
            return Err("缺少 Telegram 凭据解密密钥".into());
        }
        crate::feishu_personal::open_app_secret(key, ciphertext)
            .map_err(|_| "Telegram 凭据无法解密".to_string())?
    } else if stored.starts_with("enc1:") {
        return Err("旧版 Telegram 凭据需要迁移".into());
    } else if stored.is_empty() {
        std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default()
    } else {
        stored.to_string()
    };
    if !token.is_empty() && !telegram_token_ok(&token) {
        return Err("Telegram 配置无效".into());
    }
    Ok(token)
}

pub fn telegram_token_ok(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else {
        return false;
    };
    (6..=16).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && (20..=128).contains(&secret.len())
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub(crate) fn chat_id_ok(chat_id: &str) -> bool {
    let digits = chat_id.strip_prefix('-').unwrap_or(chat_id);
    (1..=20).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
}

pub fn parse_telegram_bind(me_body: &str, updates_body: &str) -> Result<(String, String), String> {
    let me: serde_json::Value =
        serde_json::from_str(me_body).map_err(|_| "token 无效：响应无效".to_string())?;
    if me.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Err("token 无效".into());
    }
    let username = me["result"]["username"].as_str().unwrap_or("").to_string();
    let updates: serde_json::Value =
        serde_json::from_str(updates_body).map_err(|_| "获取会话失败：响应无效".to_string())?;
    if updates.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Err("获取会话失败".into());
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
    .map_err(|_| "无法连接 Telegram".to_string())?
    .map_err(|err| format!("无法连接 Telegram：{err}"))?;
    parse_telegram_bind(&me, &updates)
}

fn validate_telegram_send(token: &str, chat_id: &str) -> Result<(), String> {
    if telegram_token_ok(token) && chat_id_ok(chat_id) {
        Ok(())
    } else {
        Err("Telegram 配置无效".into())
    }
}

#[derive(Debug)]
struct TelegramPost {
    #[allow(dead_code)]
    id: i64,
    platform: String,
    kol_id: i64,
    kol_name: String,
    category: String,
    title: String,
    content: String,
    post_type: String,
    #[allow(dead_code)]
    images: Value,
    tags: Value,
    detail: Value,
    title_src: String,
    content_src: String,
    url: String,
    published_at: String,
}

impl TelegramPost {
    fn from_row(row: sqlx::sqlite::SqliteRow) -> Self {
        fn parsed(raw: String) -> Value {
            serde_json::from_str(&raw).unwrap_or(Value::Null)
        }
        Self {
            id: row.get("id"),
            platform: row.get("platform"),
            kol_id: row.get("kol_id"),
            kol_name: row.get("kol_name"),
            category: row.get("category"),
            title: row.get("title"),
            content: row.get("content"),
            post_type: row.get("post_type"),
            images: parsed(row.get("images")),
            tags: parsed(row.get("tags")),
            detail: parsed(row.get("detail")),
            title_src: row.get("title_src"),
            content_src: row.get("content_src"),
            url: row.get("url"),
            published_at: row.get("published_at"),
        }
    }
}

const TELEGRAM_POST_SELECT: &str =
    "SELECT p.id, p.platform, p.kol_id, p.title, p.content, p.post_type,
    p.images, p.tags, p.detail, p.title_src, p.content_src, p.url, p.published_at,
    COALESCE(k.name, '') AS kol_name, COALESCE(c.name, '') AS category
    FROM posts p LEFT JOIN kols k ON k.id = p.kol_id
    LEFT JOIN categories c ON c.id = k.category_id";

async fn load_telegram_post(db: &Db, post_id: i64) -> Result<Option<TelegramPost>, sqlx::Error> {
    let row = sqlx::query(&format!("{TELEGRAM_POST_SELECT} WHERE p.id = ?"))
        .bind(post_id)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.map(TelegramPost::from_row))
}

async fn load_telegram_identity(
    db: &Db,
    kol_id: i64,
    platform: &str,
    external_id: &str,
) -> Result<Option<TelegramPost>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "{TELEGRAM_POST_SELECT} WHERE p.platform = ? AND p.external_id = ? AND p.kol_id = ?"
    ))
    .bind(platform)
    .bind(external_id)
    .bind(kol_id)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.map(TelegramPost::from_row))
}

fn telegram_url(raw: &str) -> bool {
    url::Url::parse(raw).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && !raw.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    })
}

fn telegram_media_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return false;
    }
    let host = url.host_str().unwrap_or_default().trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return false;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast())
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            !(ip.to_ipv4().is_some_and(|mapped| {
                mapped.is_private()
                    || mapped.is_loopback()
                    || mapped.is_link_local()
                    || mapped.is_unspecified()
                    || mapped.is_broadcast()
                    || mapped.is_multicast()
            }) || ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast())
        }
        Err(_) => true,
    }
}

fn telegram_video_url(raw: &str) -> bool {
    let path = raw
        .split(['?', '#'])
        .next()
        .unwrap_or(raw)
        .to_ascii_lowercase();
    path.ends_with(".mp4") || path.ends_with(".webm")
}

struct TelegramMedia {
    rich_images: Vec<String>,
    fallback_images: Vec<String>,
    videos: Vec<String>,
}

fn telegram_post_media(post: &TelegramPost) -> TelegramMedia {
    let rich_images = post
        .images
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|url| telegram_media_url(url) && !telegram_video_url(url))
        .take(9)
        .map(str::to_owned)
        .collect();
    let first_four = post
        .images
        .as_array()
        .into_iter()
        .flatten()
        .take(4)
        .filter_map(Value::as_str)
        .filter(|url| telegram_media_url(url));
    let fallback_images = first_four
        .clone()
        .filter(|url| !telegram_video_url(url))
        .map(str::to_owned)
        .collect();
    let videos = first_four
        .filter(|url| telegram_video_url(url))
        .map(str::to_owned)
        .collect();
    TelegramMedia {
        rich_images,
        fallback_images,
        videos,
    }
}

fn telegram_rich_media_body(
    chat_id: &str,
    html: &str,
    media: &[String],
    reply_markup: Option<&Value>,
) -> String {
    let imgs = media
        .iter()
        .enumerate()
        .map(|(i, _)| format!("<img src=\"tg://photo?id=p{i}\">"))
        .collect::<String>();
    let media_html = if media.len() == 1 {
        format!("<figure>{imgs}</figure>")
    } else if media.len() > 1 {
        format!("<tg-collage>{imgs}</tg-collage>")
    } else {
        String::new()
    };
    let mut rich_message = json!({
        "html": format!("{html}{media_html}"),
        "skip_entity_detection": true,
    });
    if !media.is_empty() {
        rich_message["media"] = json!(media
            .iter()
            .enumerate()
            .map(|(i, url)| json!({
                "id": format!("p{i}"),
                "media": {"type": "photo", "media": url},
            }))
            .collect::<Vec<_>>());
    }
    let mut body = json!({
        "chat_id": chat_id,
        "rich_message": rich_message.to_string(),
    });
    if let Some(markup) = reply_markup {
        body["reply_markup"] = json!(markup.to_string());
    }
    body.to_string()
}

fn escape_html(raw: &str, max_chars: usize, max_html: usize) -> String {
    let mut out = String::new();
    for ch in raw.chars().take(max_chars) {
        let escaped = match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => {
                if out.encode_utf16().count() + ch.len_utf16() > max_html {
                    break;
                }
                out.push(ch);
                continue;
            }
        };
        if out.encode_utf16().count() + escaped.len() > max_html {
            break;
        }
        out.push_str(escaped);
    }
    out
}

fn add_html(out: &mut String, fragment: &str) {
    if out.encode_utf16().count() + fragment.encode_utf16().count() <= 4096 {
        out.push_str(fragment);
    }
}

fn detail_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn render_telegram_post(
    post: &TelegramPost,
    favorite: bool,
    keyword: bool,
) -> (String, Option<Value>) {
    let mut html = String::new();
    let kind = if post.platform == "combination" && post.detail.is_object() {
        " · 调仓"
    } else if post.post_type == "reply" {
        " · 回复"
    } else {
        ""
    };
    let heading = format!(
        "📌 {} · {}{kind}",
        post.kol_name,
        platform_label(&post.platform)
    );
    add_html(
        &mut html,
        &format!("<b>{}</b>", escape_html(&heading, 180, 900)),
    );
    if favorite || keyword {
        let reason = match (favorite, keyword) {
            (true, true) => "⭐ 特别关注 · 🔎 关键词命中",
            (true, false) => "⭐ 特别关注",
            _ => "🔎 关键词命中",
        };
        add_html(&mut html, &format!("\n<i>{reason}</i>"));
    }
    if post.platform != "combination"
        && ((!post.content_src.is_empty() && post.content_src != post.content)
            || (!post.title_src.is_empty() && post.title_src != post.title))
    {
        add_html(&mut html, "\n<i>翻译自英语</i>");
    }
    if post.platform != "combination" || !post.detail.is_object() {
        let body = if !post.content.is_empty() {
            &post.content
        } else if !post.title.is_empty() {
            &post.title
        } else {
            "（无正文）"
        };
        let body = escape_html(body, 2000, 2600);
        if post.post_type == "reply" {
            add_html(&mut html, &format!("\n\n<blockquote>{body}</blockquote>"));
        } else {
            add_html(&mut html, &format!("\n\n{body}"));
        }
    }
    if post.platform == "combination" && post.detail.is_object() {
        if let Some(stats) = post.detail["stats"].as_array() {
            let labels: Vec<String> = stats
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    Some(format!(
                        "{} {}",
                        detail_text(pair.first()?),
                        detail_text(pair.get(1)?)
                    ))
                })
                .collect();
            if !labels.is_empty() {
                add_html(
                    &mut html,
                    &format!("\n{}", escape_html(&labels.join(" · "), 350, 600)),
                );
            }
        }
        if let Some(actions) = post.detail["actions"].as_array() {
            for action in actions.iter().take(12) {
                if !action.is_object() {
                    continue;
                }
                let kind = action["type"].as_str().unwrap_or("调整");
                let mark = match kind {
                    "清仓" => "🗑",
                    "新建" => "🆕",
                    "增持" => "➕",
                    "减持" => "➖",
                    _ => "•",
                };
                let stock = action["stock"].as_str().unwrap_or("");
                let symbol = action["symbol"].as_str().unwrap_or("");
                let name = if symbol.is_empty() {
                    stock.to_string()
                } else {
                    format!("{stock}（{symbol}）")
                };
                let prev = action["prev"].as_str().unwrap_or("0.0%");
                let target = action["target"].as_str().unwrap_or("0.0%");
                let line = format!("\n{mark} {kind}　{name}\n{prev} → {target}");
                add_html(&mut html, &escape_html(&line, 300, 500));
                if !action["price"].is_null() {
                    add_html(
                        &mut html,
                        &format!(
                            "\n成交价 {}",
                            escape_html(&detail_text(&action["price"]), 40, 120)
                        ),
                    );
                }
            }
        }
        if let Some(holdings) = post.detail["holdings"].as_array() {
            let mut printed = false;
            for holding in holdings.iter().take(15) {
                let Some(name) = holding["name"].as_str().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if holding["weight"].is_null() {
                    continue;
                }
                if !printed {
                    add_html(&mut html, "\n现有持仓");
                    printed = true;
                }
                let symbol = holding["symbol"].as_str().unwrap_or("");
                let line = format!("\n{name}（{symbol}） {}%", detail_text(&holding["weight"]));
                add_html(&mut html, &escape_html(&line, 200, 350));
            }
        }
        let cash = detail_text(&post.detail["cash"]);
        if !cash.is_empty() {
            add_html(
                &mut html,
                &format!("\n💵 现金 {}", escape_html(&cash, 40, 100)),
            );
        }
    } else {
        let mut meta = Vec::new();
        if !post.category.is_empty() {
            meta.push(format!("🗂 {}", post.category));
        }
        if let Some(tags) = post.tags.as_array() {
            meta.extend(
                tags.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .take(12)
                    .map(str::to_string),
            );
        }
        if !meta.is_empty() {
            add_html(
                &mut html,
                &format!("\n{}", escape_html(&meta.join(" · "), 350, 650)),
            );
        }
    }
    if !post.published_at.is_empty() {
        add_html(
            &mut html,
            &format!("\n🕐 {}", escape_html(&post.published_at, 80, 150)),
        );
    }
    if let Some(files) = post.detail["files"].as_array() {
        let mut linked = false;
        for file in files.iter().take(10) {
            let Some(url) = file["url"]
                .as_str()
                .filter(|url| url.chars().count() <= 1000 && telegram_url(url))
            else {
                continue;
            };
            let name = file["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("附件");
            let escaped_url = escape_html(url, 1000, usize::MAX);
            if escaped_url.encode_utf16().count() > 1500 {
                continue;
            }
            let fragment = format!(
                "\n📎 <a href=\"{}\">{}</a>",
                escaped_url,
                escape_html(name, 100, 200)
            );
            if html.encode_utf16().count() + fragment.encode_utf16().count() <= 4096 {
                add_html(&mut html, &fragment);
                linked = true;
            }
        }
        if linked {
            add_html(&mut html, "\n附件链接可能过期");
        }
    }
    let keyboard = telegram_url(&post.url)
        .then(|| json!({"inline_keyboard": [[{"text": "🔗 查看原文", "url": post.url}]]}));
    (html, keyboard)
}

async fn telegram_reasons(
    db: &Db,
    post: &TelegramPost,
    user_id: i64,
) -> Result<(bool, bool), sqlx::Error> {
    let row = sqlx::query("SELECT COALESCE(s.favorite, 0) AS favorite, COALESCE(u.keywords, '[]') AS keywords FROM users u LEFT JOIN subscriptions s ON s.user_id = u.id AND s.kol_id = ? WHERE u.id = ?")
        .bind(post.kol_id).bind(user_id).fetch_optional(db.pool()).await?;
    let favorite = row
        .as_ref()
        .is_some_and(|row| row.get::<i64, _>("favorite") != 0);
    let keywords: Vec<String> = row
        .as_ref()
        .and_then(|row| serde_json::from_str(&row.get::<String, _>("keywords")).ok())
        .unwrap_or_default();
    let searchable = format!("{} {}", post.title, post.content).to_lowercase();
    let keyword = keywords
        .iter()
        .any(|word| !word.trim().is_empty() && searchable.contains(&word.trim().to_lowercase()));
    Ok((favorite, keyword))
}

async fn telegram_rich_messages(db: &Db) -> bool {
    db.setting("config_telegram_rich_messages")
        .await
        .ok()
        .flatten()
        .as_deref()
        != Some("0")
}

async fn send_telegram_post(
    token: &str,
    chat_id: &str,
    post: Option<&TelegramPost>,
    user_id: i64,
    db: &Db,
    rich_messages: bool,
) -> Result<(), String> {
    let Some(post) = post else {
        return Err("帖子不存在".into());
    };
    let (favorite, keyword) = telegram_reasons(db, post, user_id)
        .await
        .map_err(|err| err.to_string())?;
    let (html, markup) = render_telegram_post(post, favorite, keyword);
    let media = telegram_post_media(post);
    validate_telegram_send(token, chat_id)?;
    let token = token.to_string();
    let chat_id = chat_id.to_string();
    tokio::task::spawn_blocking(move || {
        telegram_deliver_post_media(
            &token,
            &chat_id,
            &html,
            markup,
            rich_messages,
            &media.rich_images,
            &media.fallback_images,
            &media.videos,
        )
    })
    .await
    .map_err(|_| "Telegram network error".to_string())?
    .map_err(TelegramRequestError::message)
}

#[derive(Debug, PartialEq, Eq)]
enum TelegramRequestError {
    Api(String),
    RateLimited,
    Transport,
}

impl TelegramRequestError {
    fn message(self) -> String {
        match self {
            Self::Api(message) => message,
            Self::RateLimited => "Telegram rate limited".into(),
            Self::Transport => "Telegram network error".into(),
        }
    }
}

fn telegram_request_error_kind(error: &TelegramRequestError) -> &'static str {
    match error {
        TelegramRequestError::Api(_) => "api",
        TelegramRequestError::RateLimited => "rate_limited",
        TelegramRequestError::Transport => "transport",
    }
}

fn telegram_deliver_post_media_with<F>(
    chat_id: &str,
    html: &str,
    reply_markup: Option<Value>,
    rich_messages: bool,
    rich_images: &[String],
    fallback_images: &[String],
    videos: &[String],
    mut request: F,
) -> Result<(), TelegramRequestError>
where
    F: FnMut(&str, &str) -> Result<(), TelegramRequestError>,
{
    let mut rich_sent = false;
    if rich_messages {
        let body = telegram_rich_media_body(chat_id, html, rich_images, reply_markup.as_ref());
        match request("sendRichMessage", &body) {
            Ok(()) => rich_sent = true,
            Err(TelegramRequestError::Api(_) | TelegramRequestError::RateLimited) => {}
            Err(error) => return Err(error),
        }
    }
    if !rich_sent {
        let fallback = telegram_message_body(chat_id, html, Some("HTML"), reply_markup);
        request("sendMessage", &fallback)?;
        telegram_send_media_best_effort(chat_id, fallback_images, videos, &mut request);
    } else {
        for video in videos.iter().take(4) {
            let body = json!({
                "chat_id": chat_id,
                "video": video,
                "supports_streaming": true,
            })
            .to_string();
            match request("sendVideo", &body) {
                Ok(()) => {}
                Err(error) => tracing::warn!(
                    error = telegram_request_error_kind(&error),
                    "Telegram additional media send failed"
                ),
            }
        }
    }
    Ok(())
}

fn telegram_send_media_best_effort<F>(
    chat_id: &str,
    images: &[String],
    videos: &[String],
    request: &mut F,
) where
    F: FnMut(&str, &str) -> Result<(), TelegramRequestError>,
{
    let images = &images[..images.len().min(4)];
    if images.len() == 1 {
        match request(
            "sendPhoto",
            &json!({"chat_id": chat_id, "photo": images[0]}).to_string(),
        ) {
            Ok(()) => {}
            Err(error) => tracing::warn!(
                error = telegram_request_error_kind(&error),
                "Telegram additional media send failed"
            ),
        }
    } else if images.len() >= 2 {
        let media = images
            .iter()
            .map(|url| json!({"type": "photo", "media": url}))
            .collect::<Vec<_>>();
        let album = json!({"chat_id": chat_id, "media": media}).to_string();
        match request("sendMediaGroup", &album) {
            Ok(()) => {}
            Err(TelegramRequestError::Api(_)) => {
                for image in images {
                    let body = json!({"chat_id": chat_id, "photo": image}).to_string();
                    match request("sendPhoto", &body) {
                        Ok(()) => {}
                        Err(error) => tracing::warn!(
                            error = telegram_request_error_kind(&error),
                            "Telegram additional media send failed"
                        ),
                    }
                }
            }
            Err(error) => tracing::warn!(
                error = telegram_request_error_kind(&error),
                "Telegram additional media send failed"
            ),
        }
    }
    for video in videos.iter().take(4) {
        let body = json!({
            "chat_id": chat_id,
            "video": video,
            "supports_streaming": true,
        })
        .to_string();
        match request("sendVideo", &body) {
            Ok(()) => {}
            Err(error) => tracing::warn!(
                error = telegram_request_error_kind(&error),
                "Telegram additional media send failed"
            ),
        }
    }
}

#[cfg(test)]
fn telegram_deliver_post_with<F>(
    chat_id: &str,
    html: &str,
    reply_markup: Option<Value>,
    rich_messages: bool,
    request: F,
) -> Result<(), TelegramRequestError>
where
    F: FnMut(&str, &str) -> Result<(), TelegramRequestError>,
{
    telegram_deliver_post_media_with(
        chat_id,
        html,
        reply_markup,
        rich_messages,
        &[],
        &[],
        &[],
        request,
    )
}

fn telegram_deliver_post_media(
    token: &str,
    chat_id: &str,
    html: &str,
    reply_markup: Option<Value>,
    rich_messages: bool,
    rich_images: &[String],
    fallback_images: &[String],
    videos: &[String],
) -> Result<(), TelegramRequestError> {
    telegram_deliver_post_media_with(
        chat_id,
        html,
        reply_markup,
        rich_messages,
        rich_images,
        fallback_images,
        videos,
        |method, body| telegram_post_media_request(token, method, body),
    )
}

fn telegram_message_body(
    chat_id: &str,
    text: &str,
    parse_mode: Option<&str>,
    reply_markup: Option<Value>,
) -> String {
    let mut body = json!({
        "chat_id": chat_id,
        "text": if parse_mode == Some("HTML") { text.to_string() } else { truncate(text, 4000) },
        "disable_web_page_preview": true,
    });
    if parse_mode == Some("HTML") {
        body["parse_mode"] = json!("HTML");
    }
    if let Some(markup) = reply_markup {
        body["reply_markup"] = markup;
    }
    body.to_string()
}

#[derive(Debug, PartialEq, Eq)]
enum TelegramResponseError {
    RateLimited(u64),
    Api(String),
    Uncertain(String),
}

fn telegram_response(status: u16, body: &str) -> Result<(), TelegramResponseError> {
    let response: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if status == 429
        || (response.get("ok").and_then(Value::as_bool) == Some(false)
            && response.get("error_code").and_then(Value::as_u64) == Some(429))
    {
        let seconds = response
            .pointer("/parameters/retry_after")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .clamp(1, 60);
        return Err(TelegramResponseError::RateLimited(seconds));
    }
    if !(200..300).contains(&status) {
        let message = format!("Telegram HTTP {status}");
        if (500..600).contains(&status) {
            return Err(TelegramResponseError::Uncertain(message));
        }
        return Err(TelegramResponseError::Api(message));
    }
    match response.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(()),
        Some(false) => Err(TelegramResponseError::Api(format!(
            "Telegram HTTP {status} response rejected"
        ))),
        None => Err(TelegramResponseError::Api(format!(
            "Telegram HTTP {status} response invalid"
        ))),
    }
}

pub(crate) fn parse_telegram_response(status: u16, body: &str) -> Result<(), String> {
    telegram_response(status, body).map_err(|err| match err {
        TelegramResponseError::RateLimited(seconds) => {
            format!("Telegram HTTP 429 rate limited; retry_after={seconds}s")
        }
        TelegramResponseError::Api(message) | TelegramResponseError::Uncertain(message) => message,
    })
}

fn telegram_request_with<F, S>(mut request: F, mut sleep: S) -> Result<String, TelegramRequestError>
where
    F: FnMut() -> Result<(u16, String), TelegramRequestError>,
    S: FnMut(Duration),
{
    for attempt in 0..2 {
        let (status, body) = request()?;
        match telegram_response(status, &body) {
            Ok(()) => return Ok(body),
            Err(TelegramResponseError::RateLimited(seconds)) if attempt == 0 => {
                sleep(Duration::from_secs(seconds));
            }
            Err(TelegramResponseError::RateLimited(_)) => {
                return Err(TelegramRequestError::RateLimited);
            }
            Err(TelegramResponseError::Api(message)) => {
                return Err(TelegramRequestError::Api(message));
            }
            Err(TelegramResponseError::Uncertain(_)) => {
                return Err(TelegramRequestError::Transport);
            }
        }
    }
    unreachable!("Telegram request loop returns on the second attempt")
}

static TELEGRAM_REQUESTS: OnceLock<Mutex<VecDeque<Instant>>> = OnceLock::new();

fn telegram_slot_at(requests: &mut VecDeque<Instant>, now: Instant) -> Option<Duration> {
    while requests
        .front()
        .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(1))
    {
        requests.pop_front();
    }
    if requests.len() >= 15 {
        return requests
            .front()
            .map(|at| Duration::from_secs(1).saturating_sub(now.duration_since(*at)));
    }
    requests.push_back(now);
    None
}

fn telegram_wait_for_slot() {
    let requests = TELEGRAM_REQUESTS.get_or_init(|| Mutex::new(VecDeque::new()));
    loop {
        let wait = {
            let mut requests = requests.lock().unwrap_or_else(|err| err.into_inner());
            telegram_slot_at(&mut requests, Instant::now())
        };
        match wait {
            Some(delay) => thread::sleep(delay),
            None => return,
        }
    }
}

fn telegram_http_response(response: ureq::Response) -> Result<(u16, String), TelegramRequestError> {
    let status = response.status();
    let body = response
        .into_string()
        .map_err(|_| TelegramRequestError::Transport)?;
    Ok((status, body))
}

fn telegram_post(token: &str, method: &str, body: &str) -> Result<(), TelegramRequestError> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .build();
    telegram_request_with(
        || {
            telegram_wait_for_slot();
            match agent
                .post(&url)
                .set("Content-Type", "application/json")
                .send_string(body)
            {
                Ok(response) | Err(ureq::Error::Status(_, response)) => {
                    telegram_http_response(response)
                }
                Err(_) => Err(TelegramRequestError::Transport),
            }
        },
        thread::sleep,
    )
    .map(|_| ())
}

const TELEGRAM_PHOTO_LIMIT: usize = 10_000_000;
const TELEGRAM_VIDEO_LIMIT: usize = 50_000_000;

#[derive(Clone, Debug)]
struct TelegramUpload {
    field: &'static str,
    filename: &'static str,
    mime: &'static str,
    bytes: Vec<u8>,
}

fn telegram_multipart_body(fields: &[(&str, &str)], files: &[TelegramUpload]) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "vpush-telegram-multipart-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    for file in files {
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n", file.field, file.filename, file.mime).as_bytes(),
        );
        body.extend_from_slice(&file.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (BOUNDARY.to_string(), body)
}

fn telegram_post_multipart(
    token: &str,
    method: &str,
    fields: &[(&str, &str)],
    files: &[TelegramUpload],
) -> Result<(), TelegramRequestError> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let (boundary, body) = telegram_multipart_body(fields, files);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build();
    telegram_request_with(
        || {
            telegram_wait_for_slot();
            match agent
                .post(&url)
                .set(
                    "Content-Type",
                    &format!("multipart/form-data; boundary={boundary}"),
                )
                .send_bytes(&body)
            {
                Ok(response) | Err(ureq::Error::Status(_, response)) => {
                    telegram_http_response(response)
                }
                Err(_) => Err(TelegramRequestError::Transport),
            }
        },
        thread::sleep,
    )
    .map(|_| ())
}

fn telegram_validate_download_url(url: &str) -> Result<(), ()> {
    let parsed = url::Url::parse(url).map_err(|_| ())?;
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") || crate::url_guard::validate_url(url, scheme).is_err() {
        return Err(());
    }
    Ok(())
}

fn telegram_download_media(url: &str, video: bool) -> Result<TelegramUpload, ()> {
    telegram_validate_download_url(url)?;
    let response = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(120))
        .redirects(0)
        .build()
        .get(url)
        .call()
        .map_err(|_| ())?;
    if !(200..300).contains(&response.status()) {
        return Err(());
    }
    let mime = response
        .header("Content-Type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let limit = if video {
        TELEGRAM_VIDEO_LIMIT
    } else {
        TELEGRAM_PHOTO_LIMIT
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if !telegram_media_body_valid(&mime, &bytes, video) {
        return Err(());
    }
    let (field, filename, mime) = if video {
        if mime == "video/webm" {
            ("video", "video.webm", "video/webm")
        } else {
            ("video", "video.mp4", "video/mp4")
        }
    } else {
        match mime.as_str() {
            "image/jpeg" => ("photo", "photo.jpg", "image/jpeg"),
            "image/png" => ("photo", "photo.png", "image/png"),
            "image/gif" => ("photo", "photo.gif", "image/gif"),
            "image/webp" => ("photo", "photo.webp", "image/webp"),
            _ => return Err(()),
        }
    };
    Ok(TelegramUpload {
        field,
        filename,
        mime,
        bytes,
    })
}

fn telegram_media_size_valid(bytes: &[u8], video: bool) -> bool {
    let limit = if video {
        TELEGRAM_VIDEO_LIMIT
    } else {
        TELEGRAM_PHOTO_LIMIT
    };
    !bytes.is_empty() && bytes.len() <= limit
}

fn telegram_media_body_valid(mime: &str, bytes: &[u8], video: bool) -> bool {
    if !telegram_media_size_valid(bytes, video) {
        return false;
    }
    if video {
        return matches!(mime, "video/mp4" | "video/webm")
            && if mime == "video/mp4" {
                bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
            } else {
                bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
            };
    }
    match mime {
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

fn telegram_post_media_with_fallback<F, D, U>(
    method: &str,
    body: &str,
    mut request: F,
    mut download: D,
    mut upload: U,
) -> Result<(), TelegramRequestError>
where
    F: FnMut(&str, &str) -> Result<(), TelegramRequestError>,
    D: FnMut(&str, bool) -> Result<TelegramUpload, ()>,
    U: FnMut(&str, &[(&str, &str)], &[TelegramUpload]) -> Result<(), TelegramRequestError>,
{
    match request(method, body) {
        Ok(()) => Ok(()),
        Err(TelegramRequestError::Api(_))
            if matches!(method, "sendPhoto" | "sendVideo" | "sendMediaGroup") =>
        {
            let payload: Value = serde_json::from_str(body)
                .map_err(|_| TelegramRequestError::Api("Telegram request rejected".into()))?;
            let chat = payload["chat_id"].as_str().unwrap_or_default();
            match method {
                "sendPhoto" | "sendVideo" => {
                    let video = method == "sendVideo";
                    let url = payload[if video { "video" } else { "photo" }]
                        .as_str()
                        .unwrap_or_default();
                    let file = download(url, video).map_err(|_| {
                        TelegramRequestError::Api("Telegram media download failed".into())
                    })?;
                    let mut fields = vec![("chat_id", chat)];
                    if video {
                        fields.push(("supports_streaming", "true"));
                    }
                    upload(method, &fields, &[file])
                }
                "sendMediaGroup" => {
                    let urls: Vec<&str> = payload["media"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|item| item["media"].as_str())
                        .take(4)
                        .collect();
                    let mut files = Vec::new();
                    for url in urls {
                        if let Ok(file) = download(url, false) {
                            files.push(file);
                        }
                    }
                    if files.is_empty() {
                        return Err(TelegramRequestError::Api(
                            "Telegram media download failed".into(),
                        ));
                    }
                    if files.len() == 1 {
                        return upload("sendPhoto", &[("chat_id", chat)], &files);
                    }
                    let media = files
                        .iter()
                        .enumerate()
                        .map(|(i, file)| {
                            let mut file = TelegramUpload {
                                field: "photo",
                                ..file.clone()
                            };
                            file.field = match i {
                                0 => "p0",
                                1 => "p1",
                                2 => "p2",
                                _ => "p3",
                            };
                            file
                        })
                        .collect::<Vec<_>>();
                    let media_json = serde_json::to_string(
                        &media
                            .iter()
                            .enumerate()
                            .map(|(i, _)| json!({"type":"photo", "media":format!("attach://p{i}")}))
                            .collect::<Vec<_>>(),
                    )
                    .unwrap_or_default();
                    let fields = [("chat_id", chat), ("media", media_json.as_str())];
                    match upload("sendMediaGroup", &fields, &media) {
                        Ok(()) => Ok(()),
                        Err(TelegramRequestError::Api(_)) => {
                            for file in &media {
                                let photo = TelegramUpload {
                                    field: "photo",
                                    ..file.clone()
                                };
                                upload(
                                    "sendPhoto",
                                    &[("chat_id", chat)],
                                    std::slice::from_ref(&photo),
                                )?;
                            }
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                _ => unreachable!(),
            }
        }
        Err(error) => Err(error),
    }
}

fn telegram_post_media_request(
    token: &str,
    method: &str,
    body: &str,
) -> Result<(), TelegramRequestError> {
    telegram_post_media_with_fallback(
        method,
        body,
        |method, body| telegram_post(token, method, body),
        telegram_download_media,
        |method, fields, files| telegram_post_multipart(token, method, fields, files),
    )
}

async fn send_telegram(token: &str, chat_id: &str, text: &str) -> Result<(), String> {
    validate_telegram_send(token, chat_id)?;
    let body = telegram_message_body(chat_id, text, None, None);
    let token = token.to_string();
    tokio::task::spawn_blocking(move || telegram_post(&token, "sendMessage", &body))
        .await
        .map_err(|_| "Telegram network error".to_string())?
        .map_err(TelegramRequestError::message)
}

fn telegram_get(token: &str, method: &str) -> Result<String, String> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .build();
    telegram_request_with(
        || {
            telegram_wait_for_slot();
            match agent.get(&url).call() {
                Ok(response) | Err(ureq::Error::Status(_, response)) => {
                    telegram_http_response(response)
                }
                Err(_) => Err(TelegramRequestError::Transport),
            }
        },
        thread::sleep,
    )
    .map_err(TelegramRequestError::message)
}

fn valid_bark_key(key: &str) -> bool {
    let len = key.len();
    (10..=100).contains(&len)
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn plain(note: &Note<'_>) -> String {
    let kind = if note.post_type == "reply" {
        " · 回复"
    } else {
        ""
    };
    let body = if note.content.is_empty() {
        note.title
    } else {
        note.content
    };
    let body = if body.is_empty() {
        "（无正文）"
    } else {
        body
    };
    let mut text = format!(
        "{} · {}{kind}\n\n{}",
        note.kol_name,
        platform_label(note.platform),
        truncate(body, 1600)
    );
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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
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
        request
            .set("Content-Type", "application/json")
            .send_string(body)
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
    use base64::Engine;

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

    fn telegram_fixture(platform: &str) -> TelegramPost {
        TelegramPost {
            id: 1,
            kol_id: 2,
            platform: platform.into(),
            kol_name: "甲<&".into(),
            category: "投资<观察>".into(),
            title: "原标题".into(),
            content: "你好<&> \"世界\"".into(),
            post_type: "reply".into(),
            images: json!(["https://example.test/image.jpg"]),
            tags: json!(["A&B", "中文"]),
            detail: json!({"files": [{"name": "财报<&>", "url": "https://example.test/a?x=1&y=2"}]}),
            title_src: "Original".into(),
            content_src: "Hello".into(),
            url: "https://example.test/post".into(),
            published_at: "2026-09-28 12:00".into(),
        }
    }

    #[test]
    fn telegram_reply_golden_html_and_keyboard() {
        let (html, keyboard) = render_telegram_post(&telegram_fixture("xueqiu"), true, true);
        assert_eq!(
            html,
            concat!(
            "<b>📌 甲&lt;&amp; · 雪球 · 回复</b>\n<i>⭐ 特别关注 · 🔎 关键词命中</i>",
            "\n<i>翻译自英语</i>\n\n<blockquote>你好&lt;&amp;&gt; &quot;世界&quot;</blockquote>",
            "\n🗂 投资&lt;观察&gt; · A&amp;B · 中文\n🕐 2026-09-28 12:00",
            "\n📎 <a href=\"https://example.test/a?x=1&amp;y=2\">财报&lt;&amp;&gt;</a>",
            "\n附件链接可能过期"
        )
        );
        assert_eq!(
            keyboard,
            Some(
                json!({"inline_keyboard": [[{"text": "🔗 查看原文", "url": "https://example.test/post"}]]})
            )
        );
        let payload: Value =
            serde_json::from_str(&telegram_message_body("-12", &html, Some("HTML"), keyboard))
                .unwrap();
        assert_eq!(payload["parse_mode"], "HTML");
        assert_eq!(payload["text"], html);
        assert_eq!(
            payload["reply_markup"]["inline_keyboard"][0][0]["url"],
            "https://example.test/post"
        );
    }

    #[test]
    fn telegram_combination_golden_html() {
        let mut post = telegram_fixture("combination");
        post.post_type = "post".into();
        post.detail = json!({
            "stats": [["今日", "+1.2%"], ["净值", "1.031"]],
            "actions": [{"type": "增持", "stock": "甲<&", "symbol": "SH1", "prev": "1%", "target": "2%", "price": "10<&"}],
            "holdings": [{"name": "乙", "symbol": "SZ2", "weight": 12.5}], "cash": "20%"
        });
        let (html, _) = render_telegram_post(&post, false, false);
        assert_eq!(
            html,
            concat!(
                "<b>📌 甲&lt;&amp; · 雪球组合 · 调仓</b>",
                "\n今日 +1.2% · 净值 1.031\n➕ 增持　甲&lt;&amp;（SH1）\n1% → 2%",
                "\n成交价 10&lt;&amp;\n现有持仓\n乙（SZ2） 12.5%",
                "\n💵 现金 20%\n🕐 2026-09-28 12:00"
            )
        );
    }

    #[test]
    fn telegram_invalid_urls_and_long_html_are_safe() {
        let mut post = telegram_fixture("twitter");
        post.url = "https://valid.test@evil.test/post".into();
        post.detail = json!({"files": [{"name": "unsafe", "url": "javascript:alert(1)"}]});
        post.content = "<&".repeat(5000);
        let (html, keyboard) = render_telegram_post(&post, false, false);
        assert!(keyboard.is_none());
        assert!(!html.contains("unsafe"));
        assert!(html.chars().count() <= 4096);
        assert!(html.encode_utf16().count() <= 4096);
        assert!(!html.contains("<&"));
        assert!(!html.ends_with("&am"));
        assert!(html.contains("🕐 2026-09-28"));
    }

    #[tokio::test]
    async fn remember_failure_surfaces_closed_database_errors() {
        let path = std::env::temp_dir().join(format!(
            "vpush-push-closed-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.pool().close().await;
        let err = remember_failure(
            &db,
            1,
            ("xueqiu", "missing"),
            "telegram",
            1,
            "send failed",
            1_000,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("closed") || err.to_string().contains("Pool"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn telegram_initial_and_retry_load_the_same_persisted_post() {
        let path = std::env::temp_dir().join(format!(
            "vpush-tg-render-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let kol = db
            .add_kol("xueqiu", "甲<&", "fixture", None, false, false, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, keywords) VALUES ('reader', 'x', '[\"你好\"]')").execute(db.pool()).await.unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'reader'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id, favorite) VALUES (?, ?, 1)")
            .bind(user)
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, content, post_type, images, tags, detail, title_src, content_src, url, published_at) VALUES ('xueqiu', ?, 'fixture', '原标题', '你好<&>', 'reply', '[\"https://example.test/a.jpg\"]', '[\"中文\"]', '{\"files\":[]}', 'Original', 'Hello', 'https://example.test/post', '2026-09-28')")
            .bind(kol).execute(db.pool()).await.unwrap();
        let initial = load_telegram_identity(&db, kol, "xueqiu", "fixture")
            .await
            .unwrap()
            .unwrap();
        let retry = load_telegram_post(&db, initial.id).await.unwrap().unwrap();
        let reasons = telegram_reasons(&db, &initial, user).await.unwrap();
        assert_eq!(reasons, (true, true));
        assert_eq!(
            render_telegram_post(&initial, reasons.0, reasons.1),
            render_telegram_post(&retry, reasons.0, reasons.1)
        );
        assert!(load_telegram_identity(&db, kol, "xueqiu", "wrong")
            .await
            .unwrap()
            .is_none());
        sqlx::query("DELETE FROM posts WHERE id = ?")
            .bind(initial.id)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(load_telegram_post(&db, initial.id).await.unwrap().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn telegram_identity_survives_newer_post_with_identical_or_empty_url() {
        let path = std::env::temp_dir().join(format!(
            "vpush-tg-identity-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let kol = db
            .add_kol("xueqiu", "甲", "identity", None, false, false, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, telegram_bot_token, telegram_chat_id, push_channels) VALUES ('reader', 'x', '123456:ABCDEFGHIJKLMNOPQRST', 'invalid-chat', 'telegram')")
            .execute(db.pool())
            .await
            .unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'reader'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id) VALUES (?, ?)")
            .bind(user)
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        for (id, content) in [("A", "A 正文"), ("B", "B 正文")] {
            sqlx::query("INSERT INTO posts (platform, kol_id, external_id, content, url) VALUES ('xueqiu', ?, ?, ?, '')")
                .bind(kol).bind(id).bind(content).execute(db.pool()).await.unwrap();
        }
        let a_id: i64 = sqlx::query_scalar(
            "SELECT id FROM posts WHERE platform = 'xueqiu' AND external_id = 'A'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let a = load_telegram_identity(&db, kol, "xueqiu", "A")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a.id, a_id);
        let (html, _) = render_telegram_post(&a, false, false);
        assert!(html.contains("A 正文"));
        assert!(!html.contains("B 正文"));
        let note = Note {
            kol_name: "甲",
            platform: "xueqiu",
            external_id: "A",
            post_type: "post",
            title: "A",
            content: "A 正文",
            url: "",
            published_at: "",
        };
        deliver(&db, kol, &note).await;
        let retry_id: i64 =
            sqlx::query_scalar("SELECT post_id FROM push_retries WHERE channel = 'telegram'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(retry_id, a_id);
        let retry = load_telegram_post(&db, retry_id).await.unwrap().unwrap();
        assert_eq!(
            render_telegram_post(&a, false, false),
            render_telegram_post(&retry, false, false)
        );
        sqlx::query("UPDATE posts SET url = 'https://example.test/same' WHERE kol_id = ?")
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            load_telegram_identity(&db, kol, "xueqiu", "A")
                .await
                .unwrap()
                .unwrap()
                .id,
            a_id
        );
        assert!(load_telegram_identity(&db, kol + 1, "xueqiu", "A")
            .await
            .unwrap()
            .is_none());
        sqlx::query("DELETE FROM posts WHERE id = ?")
            .bind(a_id)
            .execute(db.pool())
            .await
            .unwrap();
        let next_at: i64 = sqlx::query_scalar("SELECT next_at FROM push_retries WHERE post_id = ?")
            .bind(a_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(retry_due_live(&db, next_at).await.unwrap(), 0);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM push_logs WHERE post_id = ? AND channel = 'telegram'",
        )
        .bind(a_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(status, "failed");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn telegram_missing_post_or_lookup_error_never_sends_or_queues_another_post() {
        let path = std::env::temp_dir().join(format!(
            "vpush-tg-missing-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let kol = db
            .add_kol("xueqiu", "甲", "missing", None, false, false, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, telegram_bot_token, telegram_chat_id, push_channels) VALUES ('reader', 'x', '123456:ABCDEFGHIJKLMNOPQRST', 'invalid-chat', 'telegram')")
            .execute(db.pool()).await.unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'reader'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id) VALUES (?, ?)")
            .bind(user)
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id) VALUES ('xueqiu', ?, 'B')")
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(load_telegram_identity(&db, kol, "xueqiu", "A")
            .await
            .unwrap()
            .is_none());
        let note = Note {
            kol_name: "甲",
            platform: "xueqiu",
            external_id: "A",
            post_type: "post",
            title: "A",
            content: "A 正文",
            url: "",
            published_at: "",
        };
        deliver(&db, kol, &note).await;
        assert!(
            send_telegram_post("123456:ABCDEFGHIJKLMNOPQRST", "-1", None, user, &db, true)
                .await
                .is_err()
        );
        remember_failure(&db, kol, ("xueqiu", "A"), "telegram", 1, "missing", 1_000)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_retries")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
        sqlx::query("DROP TABLE posts")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(load_telegram_identity(&db, kol, "xueqiu", "A")
            .await
            .is_err());
        deliver(&db, kol, &note).await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_retries")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn telegram_secret_requires_migration_for_legacy_ciphertext() {
        let key = base64::engine::general_purpose::URL_SAFE.encode([3u8; 32]);
        assert!(telegram_secret("enc1:legacy-ciphertext", &key)
            .unwrap_err()
            .contains("迁移"));
    }

    #[test]
    fn telegram_secret_decrypts_enc2_and_accepts_plaintext() {
        use base64::Engine;

        let key = base64::engine::general_purpose::URL_SAFE.encode([3u8; 32]);
        let token = "123456:ABCDEFGHIJKLMNOPQRST";
        let encrypted = crate::feishu_personal::seal(&key, token).unwrap();
        assert_eq!(
            telegram_secret(&format!("enc2:{encrypted}"), &key).unwrap(),
            token
        );
        assert_eq!(telegram_secret(token, &key).unwrap(), token);
    }

    #[test]
    fn telegram_secret_does_not_fallback_when_key_is_missing_or_wrong() {
        use base64::Engine;

        let key = base64::engine::general_purpose::URL_SAFE.encode([3u8; 32]);
        let other_key = base64::engine::general_purpose::URL_SAFE.encode([4u8; 32]);
        let encrypted = crate::feishu_personal::seal(&key, "123456:ABCDEFGHIJKLMNOPQRST").unwrap();
        assert!(telegram_secret(&format!("enc2:{encrypted}"), "").is_err());
        assert!(telegram_secret(&format!("enc2:{encrypted}"), &other_key).is_err());
        assert!(telegram_secret("not-a-telegram-token", &key).is_err());
    }

    #[test]
    fn telegram_response_requires_success_json() {
        assert!(
            parse_telegram_response(200, r#"{"ok":false,"description":"secret"}"#)
                .unwrap_err()
                .contains("rejected")
        );
        assert!(parse_telegram_response(200, "not json")
            .unwrap_err()
            .contains("invalid"));
        assert_eq!(parse_telegram_response(200, r#"{"ok":true}"#), Ok(()));
    }

    #[test]
    fn telegram_rate_limit_error_is_bounded_and_redacted() {
        let token = "123456:ABCDEFGHIJKLMNOPQRST";
        let body = serde_json::json!({
            "ok": false,
            "description": format!(
                "{token} https://api.telegram.org/bot{token} chat=-123"
            ),
            "parameters": {"retry_after": 17},
        })
        .to_string();
        let err = parse_telegram_response(429, &body).unwrap_err();
        assert_eq!(err, "Telegram HTTP 429 rate limited; retry_after=17s");
        assert!(!err.contains(token));
        assert!(!err.contains("api.telegram.org"));
        assert_eq!(
            parse_telegram_response(
                429,
                r#"{"ok":false,"parameters":{"retry_after":999999999}}"#
            )
            .unwrap_err(),
            "Telegram HTTP 429 rate limited; retry_after=60s"
        );
        let err = parse_telegram_response(500, &body).unwrap_err();
        assert!(err.starts_with("Telegram HTTP 500"));
        assert!(!err.contains(token));
        assert!(!err.contains("api.telegram.org"));
        assert!(!err.contains("-123"));
    }

    #[test]
    fn telegram_rich_delivery_uses_stringified_payload_and_preserves_html_fallback() {
        let markup =
            json!({"inline_keyboard": [[{"text": "Open", "url": "https://example.test/post"}]]});
        let html = "<b>Post &amp; title</b>";
        let mut calls = Vec::new();
        telegram_deliver_post_with("-123", html, Some(markup.clone()), true, |method, body| {
            calls.push((
                method.to_string(),
                serde_json::from_str::<Value>(body).unwrap(),
            ));
            if method == "sendRichMessage" {
                Err(TelegramRequestError::Api("Telegram HTTP 400".into()))
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "sendRichMessage");
        assert_eq!(calls[0].1["chat_id"], "-123");
        assert_eq!(
            serde_json::from_str::<Value>(calls[0].1["rich_message"].as_str().unwrap()).unwrap(),
            json!({"html": html, "skip_entity_detection": true})
        );
        assert_eq!(
            serde_json::from_str::<Value>(calls[0].1["reply_markup"].as_str().unwrap()).unwrap(),
            markup
        );
        assert_eq!(calls[1].0, "sendMessage");
        assert_eq!(calls[1].1["text"], html);
        assert_eq!(calls[1].1["parse_mode"], "HTML");
        assert_eq!(calls[1].1["reply_markup"], markup);
        assert_eq!(calls[1].1["disable_web_page_preview"], true);
    }

    #[test]
    fn telegram_rich_success_uses_only_rich_and_transport_failure_does_not_fallback() {
        let mut calls = Vec::new();
        telegram_deliver_post_with("-123", "<b>Hello</b>", None, true, |method, body| {
            calls.push((
                method.to_string(),
                serde_json::from_str::<Value>(body).unwrap(),
            ));
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sendRichMessage");
        assert!(calls[0].1.get("reply_markup").is_none());

        let mut attempts = 0;
        let result = telegram_deliver_post_with("-123", "html", None, true, |_, _| {
            attempts += 1;
            Err(TelegramRequestError::Transport)
        });
        assert_eq!(result, Err(TelegramRequestError::Transport));
        assert_eq!(attempts, 1);

        let mut disabled = Vec::new();
        telegram_deliver_post_with("-123", "html", None, false, |method, _| {
            disabled.push(method.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(disabled, ["sendMessage"]);
    }

    #[test]
    fn telegram_media_urls_split_images_and_videos_with_public_url_rules() {
        let mut post = telegram_fixture("x");
        post.images = json!([
            "https://cdn.example/image.jpg",
            "https://cdn.example/clip.mp4?token=1",
            "http://cdn.example/image-2.png",
            "https://127.0.0.1/private.jpg",
            "https://cdn.example/clip.webm#part",
            "https://user:pass@cdn.example/secret.jpg",
        ]);
        let media = telegram_post_media(&post);
        assert_eq!(
            media.rich_images,
            [
                "https://cdn.example/image.jpg",
                "http://cdn.example/image-2.png"
            ]
        );
        assert_eq!(
            media.fallback_images,
            [
                "https://cdn.example/image.jpg",
                "http://cdn.example/image-2.png"
            ]
        );
        assert_eq!(media.videos, ["https://cdn.example/clip.mp4?token=1"]);
    }

    #[test]
    fn telegram_media_sends_follow_original_first_four_window() {
        let mut post = telegram_fixture("x");
        post.images = json!([
            "https://cdn.example/photo-0.jpg",
            "https://cdn.example/photo-1.jpg",
            "https://cdn.example/photo-2.jpg",
            "https://cdn.example/photo-3.jpg",
            "https://cdn.example/photo-4.jpg",
            "https://cdn.example/late.mp4",
        ]);
        let media = telegram_post_media(&post);
        assert_eq!(media.rich_images.len(), 5);
        assert_eq!(media.fallback_images.len(), 4);
        assert!(media.videos.is_empty());
        let mut methods = Vec::new();
        telegram_deliver_post_media_with(
            "-123",
            "html",
            None,
            true,
            &media.rich_images,
            &media.fallback_images,
            &media.videos,
            |method, _| {
                methods.push(method.to_string());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(methods, ["sendRichMessage"]);

        post.images = json!([
            "https://cdn.example/photo-0.jpg",
            "https://cdn.example/interleaved.mp4",
            "https://cdn.example/photo-1.jpg",
            "https://cdn.example/photo-2.jpg",
            "https://cdn.example/photo-3.jpg",
            "https://cdn.example/photo-4.jpg",
        ]);
        let media = telegram_post_media(&post);
        assert_eq!(media.rich_images.len(), 5);
        assert_eq!(media.fallback_images.len(), 3);
        assert_eq!(media.videos, ["https://cdn.example/interleaved.mp4"]);
        let mut calls = Vec::new();
        telegram_deliver_post_media_with(
            "-123",
            "html",
            None,
            false,
            &media.rich_images,
            &media.fallback_images,
            &media.videos,
            |method, body| {
                calls.push((
                    method.to_string(),
                    serde_json::from_str::<Value>(body).unwrap(),
                ));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            ["sendMessage", "sendMediaGroup", "sendVideo"]
        );
        assert_eq!(calls[1].1["media"].as_array().unwrap().len(), 3);
        assert_eq!(calls[2].1["video"], "https://cdn.example/interleaved.mp4");
    }

    #[test]
    fn telegram_rich_media_payload_uses_matching_photo_ids_and_collage() {
        let images = vec![
            "https://cdn.example/one.jpg".to_string(),
            "https://cdn.example/two.png".to_string(),
        ];
        let body: Value = serde_json::from_str(&telegram_rich_media_body(
            "-123",
            "<b>Post</b>",
            &images,
            None,
        ))
        .unwrap();
        let rich: Value = serde_json::from_str(body["rich_message"].as_str().unwrap()).unwrap();
        assert_eq!(rich["html"], "<b>Post</b><tg-collage><img src=\"tg://photo?id=p0\"><img src=\"tg://photo?id=p1\"></tg-collage>");
        assert_eq!(
            rich["media"][0],
            json!({
                "id": "p0",
                "media": {"type": "photo", "media": "https://cdn.example/one.jpg"}
            })
        );
        assert_eq!(rich["media"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn telegram_rich_rejection_falls_back_to_text_then_media() {
        let images = vec!["https://cdn.example/one.jpg".to_string()];
        let videos = vec!["https://cdn.example/clip.mp4".to_string()];
        let mut methods = Vec::new();
        telegram_deliver_post_media_with(
            "-123",
            "<b>Post</b>",
            None,
            true,
            &images,
            &images,
            &videos,
            |method, _| {
                methods.push(method.to_string());
                if method == "sendRichMessage" {
                    Err(TelegramRequestError::Api("rejected".into()))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        assert_eq!(
            methods,
            ["sendRichMessage", "sendMessage", "sendPhoto", "sendVideo"]
        );
    }

    #[test]
    fn telegram_album_rejection_falls_back_to_each_photo() {
        let images = vec![
            "https://cdn.example/one.jpg".to_string(),
            "https://cdn.example/two.jpg".to_string(),
        ];
        let mut methods = Vec::new();
        telegram_deliver_post_media_with(
            "-123",
            "html",
            None,
            false,
            &images,
            &images,
            &[],
            |method, _| {
                methods.push(method.to_string());
                if method == "sendMediaGroup" {
                    Err(TelegramRequestError::Api("album rejected".into()))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        assert_eq!(
            methods,
            ["sendMessage", "sendMediaGroup", "sendPhoto", "sendPhoto"]
        );
    }

    #[test]
    fn telegram_album_transport_failure_does_not_resend_photos() {
        let images = vec![
            "https://cdn.example/one.jpg".to_string(),
            "https://cdn.example/two.jpg".to_string(),
        ];
        let mut methods = Vec::new();
        telegram_deliver_post_media_with(
            "-123",
            "html",
            None,
            false,
            &images,
            &images,
            &[],
            |method, _| {
                methods.push(method.to_string());
                if method == "sendMediaGroup" {
                    Err(TelegramRequestError::Transport)
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        assert_eq!(methods, ["sendMessage", "sendMediaGroup"]);
    }

    #[test]
    fn telegram_http_5xx_does_not_become_api_rejection() {
        let err =
            telegram_request_with(|| Ok((500, r#"{"ok":false}"#.into())), |_| {}).unwrap_err();
        assert_eq!(err, TelegramRequestError::Transport);
        let err =
            telegram_request_with(|| Ok((400, r#"{"ok":false}"#.into())), |_| {}).unwrap_err();
        assert!(matches!(err, TelegramRequestError::Api(_)));
    }

    #[tokio::test]
    async fn unbound_wecom_and_bark_are_errors() {
        assert_eq!(
            send_wecom_text("", "hi").await.unwrap_err(),
            "企业微信未绑定"
        );
        assert_eq!(
            send_bark_text("", "t", "b").await.unwrap_err(),
            "Bark 未绑定"
        );
    }

    #[test]
    fn telegram_multipart_builder_uses_controlled_fields_and_binary_bytes() {
        let file = TelegramUpload {
            field: "photo",
            filename: "photo.jpg",
            mime: "image/jpeg",
            bytes: vec![0xff, 0xd8, 0xff, 0x00],
        };
        let (boundary, body) = telegram_multipart_body(&[("chat_id", "-123")], &[file]);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"chat_id\""
        )));
        assert!(text.contains("name=\"photo\"; filename=\"photo.jpg\"\r\nContent-Type: image/jpeg"));
        assert!(body
            .windows(4)
            .any(|window| window == [0xff, 0xd8, 0xff, 0x00]));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn telegram_media_download_validation_rejects_private_malformed_and_oversize() {
        for url in [
            "http://127.0.0.1/a.jpg",
            "https://localhost/a.jpg",
            "https://[::1]/a.jpg",
            "https://user:pass@example.test/a.jpg",
            "not a url",
            "file:///etc/passwd",
        ] {
            assert!(telegram_validate_download_url(url).is_err(), "{url}");
        }
        assert!(telegram_media_size_valid(
            &vec![0; TELEGRAM_PHOTO_LIMIT],
            false
        ));
        assert!(!telegram_media_size_valid(
            &vec![0; TELEGRAM_PHOTO_LIMIT + 1],
            false
        ));
        assert!(telegram_media_size_valid(
            &vec![0; TELEGRAM_VIDEO_LIMIT],
            true
        ));
        assert!(!telegram_media_size_valid(
            &vec![0; TELEGRAM_VIDEO_LIMIT + 1],
            true
        ));
        assert!(!telegram_media_body_valid(
            "image/jpeg",
            b"not an image",
            false
        ));
    }

    #[test]
    fn telegram_media_fallback_is_api_rejection_only_and_builds_photo_upload() {
        let mut requests = Vec::new();
        let mut upload_methods = Vec::new();
        let body = json!({"chat_id":"-123", "photo":"https://cdn.example/a.jpg"}).to_string();
        telegram_post_media_with_fallback(
            "sendPhoto",
            &body,
            |method, _| {
                requests.push(method.to_string());
                Err(TelegramRequestError::Api("Telegram HTTP 400".into()))
            },
            |url, video| {
                assert_eq!(url, "https://cdn.example/a.jpg");
                assert!(!video);
                Ok(TelegramUpload {
                    field: "photo",
                    filename: "photo.jpg",
                    mime: "image/jpeg",
                    bytes: vec![1],
                })
            },
            |method, fields, files| {
                upload_methods.push(method.to_string());
                assert_eq!(fields, [("chat_id", "-123")]);
                assert_eq!(files[0].field, "photo");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(requests, ["sendPhoto"]);
        assert_eq!(upload_methods, ["sendPhoto"]);

        let mut downloads = 0;
        let result = telegram_post_media_with_fallback(
            "sendPhoto",
            &body,
            |_, _| Err(TelegramRequestError::Transport),
            |_, _| {
                downloads += 1;
                unreachable!()
            },
            |_, _, _| unreachable!(),
        );
        assert_eq!(result, Err(TelegramRequestError::Transport));
        assert_eq!(downloads, 0);
    }

    #[test]
    fn telegram_album_fallback_uploads_attach_group_then_individuals_on_api_rejection() {
        let urls = ["https://cdn.example/1.jpg", "https://cdn.example/2.jpg"];
        let body = json!({"chat_id":"-123", "media":urls.iter().map(|url| json!({"type":"photo", "media":url})).collect::<Vec<_>>()}).to_string();
        let mut uploads = Vec::new();
        telegram_post_media_with_fallback(
            "sendMediaGroup",
            &body,
            |_, _| Err(TelegramRequestError::Api("rejected".into())),
            |_, _| {
                Ok(TelegramUpload {
                    field: "photo",
                    filename: "photo.jpg",
                    mime: "image/jpeg",
                    bytes: vec![1],
                })
            },
            |method, fields, files| {
                uploads.push((
                    method.to_string(),
                    fields
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                        .collect::<Vec<_>>(),
                    files.iter().map(|file| file.field).collect::<Vec<_>>(),
                ));
                if method == "sendMediaGroup" {
                    Err(TelegramRequestError::Api("album rejected".into()))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        assert_eq!(uploads[0].0, "sendMediaGroup");
        assert_eq!(uploads[0].2, ["p0", "p1"]);
        assert!(uploads[0].1[1].1.contains("attach://p0"));
        assert_eq!(uploads[1].0, "sendPhoto");
        assert_eq!(uploads[2].0, "sendPhoto");
    }

    #[test]
    fn telegram_media_errors_do_not_lose_successful_text() {
        let images = vec!["https://cdn.example/one.jpg".to_string()];
        let mut methods = Vec::new();
        let result = telegram_deliver_post_media_with(
            "-123",
            "html",
            None,
            false,
            &images,
            &images,
            &[],
            |method, _| {
                methods.push(method.to_string());
                if method == "sendPhoto" {
                    Err(TelegramRequestError::Transport)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(methods, ["sendMessage", "sendPhoto"]);
    }

    #[test]
    fn telegram_request_retries_only_explicit_429_once_and_redacts_errors() {
        for first in [
            (429, r#"{"ok":false,"parameters":{"retry_after":3}}"#),
            (
                200,
                r#"{"ok":false,"error_code":429,"parameters":{"retry_after":1000}}"#,
            ),
        ] {
            let mut attempts = 0;
            let mut delays = Vec::new();
            assert!(telegram_request_with(
                || {
                    attempts += 1;
                    let (status, body) = if attempts == 1 {
                        first
                    } else {
                        (200, r#"{"ok":true}"#)
                    };
                    Ok((status, body.into()))
                },
                |duration| delays.push(duration),
            )
            .is_ok());
            assert_eq!(attempts, 2);
            assert_eq!(
                delays,
                [Duration::from_secs(if first.0 == 429 { 3 } else { 60 })]
            );
        }
        let secret = "123456:ABCDEFGHIJKLMNOPQRST https://api.telegram.org/bot123456:ABCDEFGHIJKLMNOPQRST chat=-123";
        let mut attempts = 0;
        let error = telegram_request_with(
            || {
                attempts += 1;
                Ok((429, json!({"description": secret}).to_string()))
            },
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error.message(), "Telegram rate limited");
        assert_eq!(attempts, 2);
        for (status, body) in [
            (400, json!({"ok": false, "description": secret}).to_string()),
            (200, json!({"ok": false, "description": secret}).to_string()),
        ] {
            let mut attempts = 0;
            let error = telegram_request_with(
                || {
                    attempts += 1;
                    Ok((status, body.clone()))
                },
                |_| panic!("unexpected wait"),
            )
            .unwrap_err()
            .message();
            assert_eq!(attempts, 1);
            assert!(!error.contains(secret));
            assert!(!error.contains("-123"));
        }
        let mut attempts = 0;
        assert_eq!(
            telegram_request_with(
                || {
                    attempts += 1;
                    Err(TelegramRequestError::Transport)
                },
                |_| panic!("unexpected wait"),
            ),
            Err(TelegramRequestError::Transport)
        );
        assert_eq!(attempts, 1);
        assert_eq!(
            telegram_response(429, "invalid"),
            Err(TelegramResponseError::RateLimited(1))
        );
        assert_eq!(
            telegram_response(
                200,
                r#"{"ok":false,"error_code":429,"parameters":{"retry_after":0}}"#
            ),
            Err(TelegramResponseError::RateLimited(1))
        );
    }

    #[test]
    fn telegram_global_limiter_caps_each_sliding_second() {
        let start = Instant::now();
        let mut events = VecDeque::new();
        for _ in 0..15 {
            assert_eq!(telegram_slot_at(&mut events, start), None);
        }
        assert_eq!(
            telegram_slot_at(&mut events, start),
            Some(Duration::from_secs(1))
        );
        assert_eq!(events.len(), 15);
        assert_eq!(
            telegram_slot_at(&mut events, start + Duration::from_millis(999)),
            Some(Duration::from_millis(1))
        );
        assert_eq!(
            telegram_slot_at(&mut events, start + Duration::from_secs(1)),
            None
        );
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn telegram_rich_setting_defaults_on_and_respects_off() {
        let path = std::env::temp_dir().join(format!(
            "vpush-tg-rich-setting-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        assert!(telegram_rich_messages(&db).await);
        db.set_setting("config_telegram_rich_messages", "0")
            .await
            .unwrap();
        assert!(!telegram_rich_messages(&db).await);
        db.set_setting("config_telegram_rich_messages", "1")
            .await
            .unwrap();
        assert!(telegram_rich_messages(&db).await);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn telegram_message_payload_is_plain_by_default_and_truncated_safely() {
        let body: serde_json::Value = serde_json::from_str(&telegram_message_body(
            "-123",
            &"字".repeat(5000),
            None,
            None,
        ))
        .unwrap();
        assert_eq!(body["chat_id"], "-123");
        assert!(body.get("parse_mode").is_none());
        assert!(body["text"].as_str().unwrap().chars().count() <= 4000);

        let html: serde_json::Value = serde_json::from_str(&telegram_message_body(
            "-123",
            "<b>text</b>",
            Some("HTML"),
            None,
        ))
        .unwrap();
        assert_eq!(html["parse_mode"], "HTML");
    }

    #[tokio::test]
    async fn send_telegram_rejects_unbound_configuration() {
        assert!(send_telegram("", "", "test").await.is_err());
        assert!(send_telegram("123456:ABCDEFGHIJKLMNOPQRST", "", "test")
            .await
            .is_err());
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
        assert!(wecom_bound(
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abc"
        ));
        assert!(!wecom_bound("https://evil.example/hook"));
    }

    #[tokio::test]
    async fn unbound_user_is_not_sent() {
        let path = std::env::temp_dir().join(format!(
            "vpush-push-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let mut user = db.user_by_username("admin").await.unwrap().unwrap();
        user.push_channels = "wecom".into();
        let err = test_user(&db, &user, "你好").await.unwrap_err();
        assert_eq!(err, "该用户未绑定任何推送渠道");
        user.push_channels = "telegram".into();
        user.telegram_bot_token = "123456:ABCDEFGHIJKLMNOPQRST".into();
        user.telegram_chat_id.clear();
        let err = test_user(&db, &user, "你好").await.unwrap_err();
        assert_eq!(err, "该用户未绑定任何推送渠道");
        db.set_user_text(user.id, "push_channels", "telegram")
            .await
            .unwrap();
        db.set_user_text(user.id, "telegram_bot_token", "123456:ABCDEFGHIJKLMNOPQRST")
            .await
            .unwrap();
        assert_eq!(
            send_user_text(&db, user.id, "你好").await.unwrap_err(),
            "该用户未绑定任何推送渠道"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn telegram_bind_uses_the_latest_chat_and_rejects_a_silent_bot() {
        let me = r#"{"ok":true,"result":{"username":"dav_bot"}}"#;
        let updates = r#"{"ok":true,"result":[{"message":{"chat":{"id":111}}},{"message":{"chat":{"id":-222}}}]}"#;
        assert_eq!(
            parse_telegram_bind(me, updates).unwrap(),
            ("dav_bot".into(), "-222".into())
        );
        let silent = r#"{"ok":true,"result":[]}"#;
        assert!(parse_telegram_bind(me, silent)
            .unwrap_err()
            .contains("先给你的机器人发一条消息"));
        assert!(
            parse_telegram_bind(r#"{"ok":false,"description":"Unauthorized"}"#, silent)
                .unwrap_err()
                .contains("token 无效")
        );
        assert!(telegram_token_ok("123456:ABCDEFGHIJKLMNOPQRST"));
        assert!(!telegram_token_ok("123456:short"));
        assert!(!telegram_token_ok("bad/token:ABCDEFGHIJKLMNOPQRST"));
    }

    #[tokio::test]
    async fn deliver_skips_unbound_telegram_in_all_channels() {
        let path = std::env::temp_dir().join(format!(
            "vpush-deliver-unbound-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, telegram_bot_token, push_channels) VALUES ('telegram', 'x', '123456:ABCDEFGHIJKLMNOPQRST', '')")
            .execute(db.pool())
            .await
            .unwrap();
        let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'telegram'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let kol_id = db
            .add_kol(
                "xueqiu",
                "测试",
                "deliver-unbound",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id, type) VALUES (?, ?, 'post')")
            .bind(user_id)
            .bind(kol_id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'deliver-unbound', '标题', 'https://example.test/post')")
            .bind(kol_id)
            .execute(db.pool())
            .await
            .unwrap();
        let note = Note {
            kol_name: "测试",
            platform: "xueqiu",
            external_id: "deliver-unbound",
            post_type: "post",
            title: "标题",
            content: "正文",
            url: "https://example.test/post",
            published_at: "2026-01-01 00:00",
        };
        let targets = db.push_targets(kol_id).await.unwrap();
        assert_eq!(targets.len(), 1);
        assert!(targets[0].notify_enabled);
        assert!(targets[0].telegram_chat_id.is_empty());
        assert!(telegram_token_ok(&targets[0].telegram_bot_token));
        deliver(&db, kol_id, &note).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM push_retries WHERE channel = 'telegram'",
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM push_logs WHERE channel = 'telegram'",
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            0
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn retry_keeps_invalid_telegram_delivery_pending() {
        let path = std::env::temp_dir().join(format!(
            "vpush-retry-telegram-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('telegram', 'x')")
            .execute(db.pool())
            .await
            .unwrap();
        let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'telegram'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let kol_id = db
            .add_kol(
                "xueqiu",
                "测试",
                "retry-telegram",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id, type) VALUES (?, ?, 'post')")
            .bind(user_id)
            .bind(kol_id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'retry-telegram', '标题', 'https://example.test/retry')")
            .bind(kol_id)
            .execute(db.pool())
            .await
            .unwrap();
        remember_failure(
            &db,
            kol_id,
            ("xueqiu", "retry-telegram"),
            "telegram",
            user_id,
            "Telegram 配置无效",
            1_000,
        )
        .await
        .unwrap();
        assert_eq!(
            retry_due(&db, 1_000, |_, _, _| async {
                send_telegram("", "", "retry").await
            })
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            retry_due(&db, 1_060, |_, _, _| async {
                parse_telegram_response(200, r#"{"ok":true}"#)
            })
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_logs WHERE status = 'success'")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn failed_push_retries_then_stops() {
        let path = std::env::temp_dir().join(format!(
            "vpush-retry-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query(
            "INSERT INTO users (username, password_hash, notify_enabled) VALUES ('甲', 'x', 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = '甲'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let kol = db
            .add_kol("xueqiu", "段永平", "111", None, false, false, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id, type) VALUES (?, ?, 'post')")
            .bind(user)
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'p1', '标题', 'https://xueqiu.com/p/1')").bind(kol).execute(db.pool()).await.unwrap();
        remember_failure(&db, kol, ("xueqiu", "p1"), "wecom", user, "超时", 1_000)
            .await
            .unwrap();
        remember_failure(
            &db,
            kol,
            ("xueqiu", "not-found"),
            "wecom",
            user,
            "不应入队",
            1_000,
        )
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            retry_due(&db, 1_000, |_, _, _| async { Ok(()) })
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            retry_due(&db, 1_060, |_, _, _| async { Err("仍失败".into()) })
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT attempts FROM push_retries")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            retry_due(&db, 1_060 + 300, |_, channel, _| async move {
                assert_eq!(channel, "wecom");
                Ok(())
            })
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM push_logs")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "success"
        );
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'p2', '二', 'https://xueqiu.com/p/2')").bind(kol).execute(db.pool()).await.unwrap();
        remember_failure(&db, kol, ("xueqiu", "p2"), "bark", user, "超时", 2_000)
            .await
            .unwrap();
        for delay in [60, 300, 900] {
            let at: i64 = sqlx::query_scalar("SELECT next_at FROM push_retries")
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(
                retry_due(&db, at, |_, _, _| async { Err("失败".into()) })
                    .await
                    .unwrap(),
                0
            );
            let _ = delay;
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM push_logs WHERE channel = 'bark'")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "failed"
        );
        sqlx::query("DELETE FROM subscriptions")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, url) VALUES ('xueqiu', ?, 'p3', '三', 'https://xueqiu.com/p/3')").bind(kol).execute(db.pool()).await.unwrap();
        remember_failure(&db, kol, ("xueqiu", "p3"), "telegram", user, "超时", 3_000)
            .await
            .unwrap();
        assert_eq!(
            retry_due(&db, 3_060, |_, _, _| async { Ok(()) })
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_retries")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            db.setting("stats_retry_pending").await.unwrap().as_deref(),
            Some("0")
        );
        let _ = std::fs::remove_file(&path);
    }
}
