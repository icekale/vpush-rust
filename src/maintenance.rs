//! 后台维护，分五条互不等待的循环，每轮结束后歇 60 秒：
//! 告警与重试、LLM 批处理、本地库文档入库、清理与备份、日报与提醒。
//! 每步单独计时、单独记错，一步失败不影响同轮后面的步骤。

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::db::Db;

const ROUND_GAP: Duration = Duration::from_secs(60);
const SLOW_STEP: Duration = Duration::from_secs(1);
/// 单个 LLM 批处理步骤的时间预算。到点后不再开始新条目，剩下的留给下一轮。
const LLM_STEP_BUDGET: Duration = Duration::from_secs(600);

type Round = for<'a> fn(&'a Db) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub fn spawn(db: Db) {
    every_round(db.clone(), |db| Box::pin(alerts_and_retries(db)));
    every_round(db.clone(), |db| Box::pin(llm_batches(db)));
    every_round(db.clone(), |db| Box::pin(cleanup_and_backup(db)));
    every_round(db.clone(), |db| Box::pin(local_library_documents(db)));
    every_round(db, |db| Box::pin(daily_reports(db)));
}

fn every_round(db: Db, round: Round) {
    tokio::spawn(async move {
        loop {
            round(&db).await;
            tokio::time::sleep(ROUND_GAP).await;
        }
    });
}

async fn timed<T>(group: &str, step: &str, work: impl Future<Output = T>) -> T {
    let started = Instant::now();
    let out = work.await;
    let elapsed = started.elapsed();
    let ms = elapsed.as_millis() as u64;
    if elapsed >= SLOW_STEP {
        tracing::info!(ms, "维护步骤耗时 {group}/{step}");
    } else {
        tracing::debug!(ms, "维护步骤耗时 {group}/{step}");
    }
    out
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn with_model(db: &Db, mut cfg: crate::llm::Config, key: &str) -> crate::llm::Config {
    if let Ok(Some(model)) = db.setting(key).await {
        if !model.trim().is_empty() {
            cfg.model = model;
        }
    }
    cfg
}

async fn admin_llm(db: &Db) -> Option<crate::llm::Config> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT llm_api_base, llm_api_key, llm_model, llm_api_format FROM users
         WHERE is_admin = 1 AND llm_api_base != '' AND llm_api_key != '' ORDER BY id LIMIT 1",
    )
    .fetch_optional(db.pool())
    .await
    .ok()??;
    crate::llm::runtime(
        None,
        None,
        row.get::<String, _>("llm_api_base").as_str(),
        row.get::<String, _>("llm_api_key").as_str(),
        None,
        row.get::<String, _>("llm_model").as_str(),
        None,
        row.get::<String, _>("llm_api_format").as_str(),
        true,
    )
    .ok()
}

enum Keep {
    Alive,
    Dead(String),
    Transient,
    Renewed(String),
}

async fn cookie_keepalive(db: &Db, now: i64) -> Result<(), sqlx::Error> {
    let probe_db = db.clone();
    cookie_keepalive_with(db, now, move |platform, cookie, uid| {
        let probe_db = probe_db.clone();
        let platform = platform.to_string();
        let cookie = cookie.to_string();
        let uid = uid.to_string();
        async move {
            match platform.as_str() {
                "xueqiu" => {
                    let cookie = cookie.clone();
                    let uid = uid.clone();
                    match tokio::task::spawn_blocking(move || {
                        crate::xueqiu::probe_keepalive(&cookie, &uid)
                    })
                    .await
                    .unwrap_or(crate::xueqiu::Keepalive::Transient)
                    {
                        crate::xueqiu::Keepalive::Alive => Keep::Alive,
                        crate::xueqiu::Keepalive::Dead(msg) => Keep::Dead(msg),
                        crate::xueqiu::Keepalive::Transient => Keep::Transient,
                    }
                }
                "weibo" => match crate::weibo::probe_keepalive(&probe_db, &cookie, &uid).await {
                    crate::weibo::Keepalive::Alive => Keep::Alive,
                    crate::weibo::Keepalive::Dead(msg) => Keep::Dead(msg),
                    crate::weibo::Keepalive::Transient => Keep::Transient,
                    crate::weibo::Keepalive::Renewed(cookie) => Keep::Renewed(cookie),
                },
                _ => Keep::Transient,
            }
        }
    })
    .await
}

async fn cookie_keepalive_with<F, Fut>(db: &Db, now: i64, mut probe: F) -> Result<(), sqlx::Error>
where
    F: FnMut(&str, &str, &str) -> Fut,
    Fut: std::future::Future<Output = Keep>,
{
    let interval = db
        .setting("config_cookie_keepalive_interval_seconds")
        .await?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if interval <= 0 {
        return Ok(());
    }
    let last = db
        .setting("cookie_keepalive_last_at")
        .await?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if last > 0 && now.saturating_sub(last) < interval {
        return Ok(());
    }
    db.set_setting("cookie_keepalive_last_at", &now.to_string())
        .await?;
    for (platform, key) in [("xueqiu", "xueqiu_cookie"), ("weibo", "weibo_cookie")] {
        let cookie = db.setting(key).await?.unwrap_or_default();
        if cookie.trim().is_empty() {
            continue;
        }
        let uid: Option<String> = sqlx::query_scalar(
            "SELECT external_id FROM kols WHERE platform = ? AND enabled = 1 ORDER BY id LIMIT 1",
        )
        .bind(platform)
        .fetch_optional(db.pool())
        .await?;
        let Some(uid) = uid.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        match probe(platform, cookie.trim(), uid.trim()).await {
            Keep::Alive => {
                db.set_setting(&format!("source_ok_{platform}"), &now.to_string())
                    .await?;
                db.set_setting(&format!("source_err_{platform}"), "")
                    .await?;
                db.set_setting(&format!("{key}_updated_at"), &now.to_string())
                    .await?;
            }
            Keep::Renewed(cookie) => {
                db.save_cookie(key, &cookie).await?;
                db.set_setting(&format!("source_ok_{platform}"), &now.to_string())
                    .await?;
                db.set_setting(&format!("source_err_{platform}"), "")
                    .await?;
            }
            Keep::Dead(detail) => {
                let detail: String = detail.chars().take(300).collect();
                db.set_setting(&format!("source_err_{platform}"), &detail)
                    .await?;
                alert_cookie(db, now, platform, &detail).await?;
            }
            Keep::Transient => {}
        }
    }
    Ok(())
}

async fn alert_cookie(db: &Db, now: i64, platform: &str, detail: &str) -> Result<(), sqlx::Error> {
    let last = db
        .setting("cookie_keepalive_alert_at")
        .await?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if last > 0 && now.saturating_sub(last) < 6 * 3600 {
        return Ok(());
    }
    let label = if platform == "weibo" {
        "微博"
    } else {
        "雪球"
    };
    let message = format!("⚠️ {label} cookie 保活失败：会话可能已过期或登录态被清除。请到后台更新 {label} Cookie。详情：{detail}");
    db.set_setting("cookie_keepalive_alert_at", &now.to_string())
        .await?;
    db.add_admin_log(0, "cookie_keepalive", platform, &message)
        .await?;
    tracing::warn!("{message}");
    Ok(())
}

async fn stock_alias_if_due(db: &Db) {
    let Ok(today): Result<String, _> = sqlx::query_scalar("SELECT date('now', '+8 hours')")
        .fetch_one(db.pool())
        .await
    else {
        return;
    };
    if db
        .setting("stock_alias_last_date")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some(today.as_str())
    {
        return;
    }
    match crate::tags::maintain(db, "none").await {
        Ok(_) => {}
        Err(crate::db::CatalogError::Conflict(_)) => return,
        Err(err) => {
            tracing::warn!("标签维护失败: {err:?}");
            return;
        }
    }
    if let Some(cfg) = admin_llm(db).await {
        let cfg = cfg.clone();
        match crate::tags::discover(db, move |marks| {
            let cfg = cfg.clone();
            async move {
                crate::llm::complete(&cfg, &format!("这些是帖子里的股票标记。返回 JSON 数组，每项含 name、official、is_alias。只输出 JSON。\n{marks}")).await
            }
        })
        .await
        {
            Ok(added) if added > 0 => tracing::info!(added, "股票别名"),
            Err(crate::db::CatalogError::Conflict(_)) => return,
            Err(err) => tracing::warn!("股票别名识别失败: {err:?}"),
            _ => {}
        }
    }
    let _ = db.set_setting("stock_alias_last_date", &today).await;
}

async fn alerts_and_retries(db: &Db) {
    const GROUP: &str = "告警与重试";
    timed(GROUP, "健康检查", async {
        match db.health_alerts().await {
            Ok(alerts) => {
                for message in alerts {
                    tracing::warn!("{message}");
                }
            }
            Err(err) => tracing::warn!("健康检查失败: {err}"),
        }
    })
    .await;
    timed(GROUP, "cookie 保活", async {
        if let Err(err) = cookie_keepalive(db, unix_now()).await {
            tracing::warn!("cookie 保活失败: {err}");
        }
    })
    .await;
    let now = unix_now();
    timed(GROUP, "雪球探测", async {
        if let Err(err) = crate::alerts::probe_xueqiu(db, now, |cookie, uid| {
            let cookie = cookie.to_string();
            let uid = uid.to_string();
            async move {
                let probe = tokio::task::spawn_blocking(move || {
                    crate::xueqiu::probe_keepalive(&cookie, &uid)
                })
                .await
                .unwrap_or(crate::xueqiu::Keepalive::Transient);
                match probe {
                    crate::xueqiu::Keepalive::Alive => crate::alerts::Probe::Alive,
                    crate::xueqiu::Keepalive::Dead(_) => crate::alerts::Probe::Dead,
                    crate::xueqiu::Keepalive::Transient => {
                        crate::alerts::Probe::Error("探测失败".into())
                    }
                }
            }
        })
        .await
        {
            tracing::warn!("雪球探测失败: {err}");
        }
    })
    .await;
    timed(GROUP, "中金检查", async {
        let status = match crate::cicc::from_env() {
            Some(ctl) => ctl.status_async().await,
            None => serde_json::json!({}),
        };
        if let Err(err) = crate::alerts::check_cicc(db, now, &status, |message| async move {
            crate::alerts::notify_admins(db, &message).await
        })
        .await
        {
            tracing::warn!("中金告警失败: {err}");
        }
    })
    .await;
    timed(GROUP, "数据源健康", async {
        if let Err(err) = crate::alerts::source_health(db, now, |message| async move {
            crate::alerts::notify_admins(db, &message).await
        })
        .await
        {
            tracing::warn!("数据源健康告警失败: {err}");
        }
    })
    .await;
    timed(GROUP, "推送重试", async {
        if let Err(err) = crate::push::retry_due_live(db, now).await {
            tracing::warn!("推送重试失败: {err}");
        }
    })
    .await;
}

async fn llm_batches(db: &Db) {
    const GROUP: &str = "LLM";
    timed(GROUP, "股票别名", stock_alias_if_due(db)).await;
    timed(GROUP, "翻译回填", translate_backfill(db)).await;
    timed(GROUP, "繁简转换", simplify_due(db)).await;
    let Some(cfg) = admin_llm(db).await else {
        return;
    };
    let extract_cfg = with_model(db, cfg.clone(), "report_extract_model").await;
    timed(GROUP, "研报抽取", async {
        let deadline = Instant::now() + LLM_STEP_BUDGET;
        match crate::reports::extract_due(
            db,
            unix_now(),
            deadline,
            crate::reports::read_text,
            |title, text| {
                let cfg = extract_cfg.clone();
                async move {
                    crate::llm::complete(
                        &cfg,
                        &format!(
                            "{}\n标题：{title}\n正文：{text}",
                            crate::reports::EXTRACT_PROMPT
                        ),
                    )
                    .await
                }
            },
        )
        .await
        {
            Ok(done) if done > 0 => tracing::info!(done, "研报抽取"),
            Err(err) => tracing::warn!("研报抽取失败: {err}"),
            _ => {}
        }
        if Instant::now() >= deadline {
            tracing::info!("研报抽取达到时间预算，剩余留到下一轮");
        }
    })
    .await;
    let digest_cfg = with_model(db, cfg, "ima_digest_model").await;
    timed(GROUP, "个股综述", async {
        let deadline = Instant::now() + LLM_STEP_BUDGET;
        match crate::reports::digest_due(db, unix_now(), deadline, |prompt| {
            let cfg = digest_cfg.clone();
            async move { crate::llm::complete(&cfg, &prompt).await }
        })
        .await
        {
            Ok(done) if done > 0 => tracing::info!(done, "个股综述"),
            Err(err) => tracing::warn!("个股综述失败: {err}"),
            _ => {}
        }
        if Instant::now() >= deadline {
            tracing::info!("个股综述达到时间预算，剩余留到下一轮");
        }
    })
    .await;
}

async fn translate_backfill(db: &Db) {
    if db
        .setting("config_translate_twitter_content")
        .await
        .ok()
        .flatten()
        .as_deref()
        != Some("1")
    {
        return;
    }
    let exit = crate::proxy_admin::acquire(db, "twitter")
        .await
        .ok()
        .flatten();
    let proxy = exit.as_ref().map(|item| item.url.clone());
    for platform in ["truth", "twitter"] {
        let deadline = Instant::now() + LLM_STEP_BUDGET;
        // ponytail: 20/min while the one-day backlog drains; 3 was the steady Python pace
        match crate::truth::backfill(db, platform, 20, deadline, |text| {
            let db = db.clone();
            let proxy = proxy.clone();
            async move { crate::translate::text(&db, &text, None, proxy.as_deref()).await }
        })
        .await
        {
            Ok(done) if done > 0 => tracing::info!(done, platform, "翻译回填"),
            Err(err) => tracing::warn!(platform, "翻译回填失败: {err}"),
            _ => {}
        }
        if Instant::now() >= deadline {
            tracing::info!(platform, "翻译回填达到时间预算，剩余留到下一轮");
        }
    }
}

/// 本地库 PDF 入库。切到 Rust 后这一步一直没人做，研报中心从 9/27 起停更。
async fn local_library_documents(db: &Db) {
    const GROUP: &str = "研报与文档";
    timed(GROUP, "本地库入库", async {
        let Ok(archive) = crate::ima_admin::archive_root() else {
            return;
        };
        let last = db
            .setting("ima_local_documents_at")
            .await
            .ok()
            .flatten()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        let now = unix_now();
        if last > 0 && now.saturating_sub(last) < 3600 {
            return;
        }
        match crate::ima_admin::sync_documents(db, &archive).await {
            Ok(written) => {
                let _ = db
                    .set_setting("ima_local_documents_at", &now.to_string())
                    .await;
                if written > 0 {
                    tracing::info!(written, "本地库文档入库");
                }
            }
            Err(err) => tracing::warn!("本地库文档入库失败: {}", err.detail),
        }
    })
    .await;
}

async fn cleanup_and_backup(db: &Db) {
    const GROUP: &str = "清理与备份";
    timed(GROUP, "新资讯源回填", async {
        match db.backfill_new_news_sources().await {
            Ok(added) if added > 0 => tracing::info!(added, "已为全部用户勾选新资讯源"),
            Err(err) => tracing::warn!("新资讯源回填失败: {err}"),
            _ => {}
        }
    })
    .await;
    timed(GROUP, "未激活清理", async {
        match db.purge_inactive_if_due().await {
            Ok(removed) if removed > 0 => tracing::info!(removed, "清理未激活用户"),
            Err(err) => tracing::warn!("清理未激活用户失败: {err}"),
            _ => {}
        }
    })
    .await;
    timed(GROUP, "过期数据清理", async {
        match db.prune_retention_if_due().await {
            Ok((posts, news, logs, admin)) if posts + news + logs + admin > 0 => {
                tracing::info!(posts, news, logs, admin, "清理过期数据")
            }
            Err(err) => tracing::warn!("清理过期数据失败: {err}"),
            _ => {}
        }
    })
    .await;
    timed(GROUP, "图床镜像", async {
        if let Some(cfg) = crate::imgbed::runtime(db).await {
            match crate::imgbed::process_due(
                db,
                cfg.retention_days,
                crate::imgbed::live_download,
                |url, bytes, kind| crate::imgbed::live_upload(&cfg, url, bytes, kind),
                |url| crate::imgbed::live_delete(&cfg, url),
            )
            .await
            {
                Ok(done) if done > 0 => tracing::info!(done, "图床镜像"),
                Err(err) => tracing::warn!("图床镜像失败: {err}"),
                _ => {}
            }
        }
    })
    .await;
    timed(GROUP, "代理池刷新", async {
        if let Err(err) =
            crate::proxy_admin::refresh_due(db, unix_now(), crate::proxy_admin::live_get).await
        {
            tracing::warn!("代理池刷新失败: {err}");
        }
    })
    .await;
    timed(GROUP, "定时备份", scheduled_backup(db)).await;
}

async fn daily_reports(db: &Db) {
    const GROUP: &str = "日报与提醒";
    timed(GROUP, "每日精选", async {
        match db.take_daily_reports().await {
            Ok(reports) => {
                for (user_id, text) in reports {
                    if let Err(err) = crate::push::send_user_text(db, user_id, &text).await {
                        tracing::warn!(user_id, "每日精选发送失败: {err}");
                    }
                }
            }
            Err(err) => tracing::warn!("每日精选失败: {err}"),
        }
    })
    .await;
    timed(GROUP, "关键词提醒", async {
        match db.pending_keyword_digests().await {
            Ok(digests) => {
                for digest in digests {
                    match crate::push::send_user_text(db, digest.user_id, &digest.text).await {
                        Ok(()) => {
                            if let Err(err) = db.mark_keyword_digest(&digest).await {
                                tracing::warn!(
                                    user_id = digest.user_id,
                                    "关键词提醒已发出但未能标记: {err}"
                                );
                            }
                        }
                        Err(err) => {
                            tracing::warn!(user_id = digest.user_id, "关键词提醒发送失败: {err}")
                        }
                    }
                }
            }
            Err(err) => tracing::warn!("关键词提醒失败: {err}"),
        }
    })
    .await;
}

async fn scheduled_backup(db: &Db) {
    let backup = crate::backup::run_scheduled(db).await;
    if let Ok(false) | Err(_) = &backup {
        let detail = match &backup {
            Err(err) => err.detail.to_string(),
            _ => db
                .setting("backup_last_error")
                .await
                .ok()
                .flatten()
                .filter(|item| !item.is_empty())
                .unwrap_or_else(|| "定时备份失败".into()),
        };
        if let Err(err) = crate::alerts::backup_failure(db, &detail, |message| async move {
            crate::alerts::notify_admins(db, &message).await
        })
        .await
        {
            tracing::warn!("备份失败告警失败: {err}");
        }
    }
    if let Err(err) = &backup {
        tracing::warn!("定时备份异常: {}", err.detail);
    }
}

async fn simplify_due(db: &Db) {
    use sqlx::Row;
    if db.setting("zh_simp_cursor").await.ok().flatten().as_deref() == Some("0") {
        return;
    }
    let before = db
        .setting("zh_simp_cursor")
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(i64::MAX);
    let rows = sqlx::query(
        "SELECT id, title, content, title_src, content_src FROM posts WHERE id < ? ORDER BY id DESC LIMIT 100",
    )
    .bind(before)
    .fetch_all(db.pool())
    .await;
    let Ok(rows) = rows else {
        return;
    };
    if rows.is_empty() {
        let _ = db.set_setting("zh_simp_cursor", "0").await;
        return;
    }
    let mut next = before;
    for row in rows {
        let id: i64 = row.get("id");
        next = id;
        let title: String = row.get("title");
        let content: String = row.get("content");
        let title_src: String = row.get("title_src");
        let content_src: String = row.get("content_src");
        let title_zh = crate::zh_simp::to_simplified(&title);
        let content_zh = crate::zh_simp::to_simplified(&content);
        let title_src_zh = if title_src == title {
            title_zh.clone()
        } else {
            title_src.clone()
        };
        let content_src_zh = if content_src == content {
            content_zh.clone()
        } else {
            content_src
        };
        if title_zh == title && content_zh == content && title_src_zh == title_src {
            continue;
        }
        let _ = sqlx::query(
            "UPDATE posts SET title = ?, content = ?, title_src = ?, content_src = ? WHERE id = ?",
        )
        .bind(&title_zh)
        .bind(&content_zh)
        .bind(&title_src_zh)
        .bind(&content_src_zh)
        .bind(id)
        .execute(db.pool())
        .await;
    }
    let _ = db.set_setting("zh_simp_cursor", &next.to_string()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_maintenance_backfills_purges_and_prunes_once() {
        let path = std::env::temp_dir().join(format!(
            "vpush-maint-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('甲', 'x'), ('乙', 'x'), ('丙', 'x')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE users SET created_at = datetime('now', '-10 days') WHERE username = '甲'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE users SET created_at = datetime('now', '-10 days'), last_login_at = datetime('now') WHERE username = '丙'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id) SELECT id, 1 FROM users WHERE username = '乙'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO news_sources (slug, name, internal, enabled) VALUES ('internal-a', '内部', 1, 1)")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO news_sources (slug, name, internal, enabled) VALUES ('public-a', '公开', 0, 1)")
            .execute(db.pool())
            .await
            .unwrap();
        db.set_setting("inactive_after_days", "1").await.unwrap();
        db.set_setting("inactive_purge_after_days", "1")
            .await
            .unwrap();
        db.set_setting("stats_posts_retention_days", "1")
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, fetched_at) VALUES ('xueqiu', 1, 'old', datetime('now', '-3 days')), ('xueqiu', 1, 'new', datetime('now'))")
            .execute(db.pool())
            .await
            .unwrap();
        cleanup_and_backup(&db).await;
        let grants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_news_sources")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(grants, 2);
        let names: Vec<String> = sqlx::query_scalar("SELECT username FROM users ORDER BY username")
            .fetch_all(db.pool())
            .await
            .unwrap();
        assert_eq!(names, vec!["丙".to_string(), "乙".to_string()]);
        let posts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(posts, 1);
        cleanup_and_backup(&db).await;
        let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(still, 2);
        assert!(crate::backup::run_scheduled(&db).await.unwrap());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn failed_cleanup_step_does_not_skip_the_rest_of_the_round() {
        let path = std::env::temp_dir().join(format!(
            "vpush-maint-isolated-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.set_setting("stats_posts_retention_days", "1")
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, fetched_at) VALUES ('xueqiu', 1, 'old', datetime('now', '-3 days')), ('xueqiu', 1, 'new', datetime('now'))")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DROP TABLE user_news_sources")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db.backfill_new_news_sources().await.is_err());
        cleanup_and_backup(&db).await;
        let posts: Vec<String> = sqlx::query_scalar("SELECT external_id FROM posts")
            .fetch_all(db.pool())
            .await
            .unwrap();
        assert_eq!(posts, vec!["new".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn health_alerts_once_per_day_after_three_failures() {
        let path = std::env::temp_dir().join(format!(
            "vpush-health-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO news_feeds (source_id, name, url, consecutive_failures, last_error_detail) VALUES (1, '华尔街见闻', 'https://example.com/rss', 3, '超时')")
            .execute(db.pool())
            .await
            .unwrap();
        let first = db.health_alerts().await.unwrap();
        assert_eq!(first.len(), 1);
        assert!(first[0].contains("华尔街见闻"));
        assert!(db.health_alerts().await.unwrap().is_empty());
        db.note_platform("xueqiu", Some("cookie 失效"))
            .await
            .unwrap();
        db.note_platform("xueqiu", Some("cookie 失效"))
            .await
            .unwrap();
        assert!(db.health_alerts().await.unwrap().is_empty());
        db.note_platform("xueqiu", Some("cookie 失效"))
            .await
            .unwrap();
        let alerts = db.health_alerts().await.unwrap();
        assert_eq!(
            alerts,
            vec!["平台 xueqiu 已连续失败 3 次：cookie 失效".to_string()]
        );
        assert!(db.health_alerts().await.unwrap().is_empty());
        db.note_platform("xueqiu", None).await.unwrap();
        let failures: i64 = sqlx::query_scalar(
            "SELECT consecutive_failures FROM platform_runs WHERE platform = 'xueqiu'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(failures, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn daily_report_is_once_per_day_and_follows_the_cursor() {
        let path = std::env::temp_dir().join(format!(
            "vpush-digest-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.set_setting("config_daily_report_hour", "0")
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, daily_report) VALUES ('甲', 'x', 1), ('乙', 'x', 0)")
            .execute(db.pool())
            .await
            .unwrap();
        let kol = db
            .add_kol("xueqiu", "段永平", "111", None, false, false, false)
            .await
            .unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = '甲'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id) VALUES (?, ?)")
            .bind(user)
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title, content) VALUES ('xueqiu', ?, 'a', '买入', ''), ('xueqiu', ?, 'b', '', '长文要点')")
            .bind(kol)
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        let first = db.take_daily_reports().await.unwrap();
        assert_eq!(first.len(), 1);
        assert!(first[0].1.contains("段永平"));
        assert!(first[0].1.contains("- 买入"));
        assert!(first[0].1.contains("- 长文要点"));
        sqlx::query("INSERT INTO posts (platform, kol_id, external_id, title) VALUES ('xueqiu', ?, 'c', '次日')")
            .bind(kol)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db.take_daily_reports().await.unwrap().is_empty());
        db.set_setting(&format!("daily_report_sent:{user}"), "2000-01-01")
            .await
            .unwrap();
        let next = db.take_daily_reports().await.unwrap();
        assert_eq!(next.len(), 1);
        assert!(next[0].1.contains("- 次日"));
        assert!(!next[0].1.contains("- 买入"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn keyword_digests_skip_dnd_unread_reports_and_mark_once() {
        let path = std::env::temp_dir().join(format!(
            "vpush-kw-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, keywords, keywords_match_news, keywords_match_news_since, keywords_match_reports, keywords_match_reports_since, dnd_start, dnd_end) VALUES ('甲', 'x', ?, 1, '2000-01-01', 1, '2000-01-01', '00:00', '23:59'), ('乙', 'x', ?, 1, '2000-01-01', 1, '2000-01-01', '', '')")
            .bind(r#"["AI", "ai"]"#)
            .bind(r#"["银行"]"#)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO news_sources (id, slug, name, internal) VALUES (1, 'public', '公开源', 0), (2, 'secret', '内部源', 1)").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO news_articles (id, source_id, external_id, title, summary, published_at) VALUES (1, 1, 'a', 'AI weekly', '', '2026-08-02'), (2, 1, 'b', '其他', '', '2026-08-02'), (3, 2, 'c', '内部 AI', '', '2026-08-02')").execute(db.pool()).await.unwrap();
        let yi: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = '乙'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO ima_kb_acl (group_id, user_id) VALUES ('reports', ?)")
            .bind(yi)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO ima_kb_subscriptions (user_id, group_id, created_at) VALUES (?, 'reports', 1)").bind(yi).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO ima_document_index (group_id, media_id, name, group_name, abstract, downloaded_at) VALUES ('reports', 'm1', '银行策略', '公开库', '', '2026-08-02T00:00:00+00:00'), ('secret', 'm2', '银行机密', '秘密库', '', '2026-08-02T00:00:00+00:00')").execute(db.pool()).await.unwrap();
        let first = db.pending_keyword_digests().await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].user_id, yi);
        assert!(first[0].text.contains("银行策略"));
        assert!(!first[0].text.contains("银行机密"));
        assert!(!first[0].text.contains("AI weekly"));
        db.mark_keyword_digest(&first[0]).await.unwrap();
        assert!(db.pending_keyword_digests().await.unwrap().is_empty());
        sqlx::query("UPDATE users SET dnd_start = '', dnd_end = '' WHERE username = '甲'")
            .execute(db.pool())
            .await
            .unwrap();
        let later = db.pending_keyword_digests().await.unwrap();
        assert_eq!(later.len(), 1);
        assert!(later[0].text.contains("AI weekly"));
        assert!(!later[0].text.contains("内部 AI"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn stock_alias_cleanup_runs_once_per_beijing_day() {
        let path = std::env::temp_dir().join(format!(
            "vpush-alias-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.set_setting("stock_names", r#"["贵州茅台"]"#)
            .await
            .unwrap();
        db.set_setting(
            "stock_aliases",
            r#"[{"alias":"酱香茅台","stock":"贵州茅台"},{"alias":"宁王","stock":"宁德时代"}]"#,
        )
        .await
        .unwrap();
        llm_batches(&db).await;
        let raw = db.setting("stock_aliases").await.unwrap().unwrap();
        assert!(raw.contains("酱香茅台"));
        assert!(!raw.contains("宁王"));
        let marked = db.setting("stock_alias_last_date").await.unwrap().unwrap();
        db.set_setting("stock_aliases", r#"[{"alias":"宁王","stock":"宁德时代"}]"#)
            .await
            .unwrap();
        llm_batches(&db).await;
        assert_eq!(
            db.setting("stock_alias_last_date").await.unwrap().unwrap(),
            marked
        );
        assert!(db
            .setting("stock_aliases")
            .await
            .unwrap()
            .unwrap()
            .contains("宁王"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn proxy_extract_refreshes_only_when_due() {
        let path = std::env::temp_dir().join(format!(
            "vpush-proxy-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let due = crate::proxy_admin::create_pool(
            &db,
            "到期",
            "extract",
            "http",
            "https://example.com/a",
            60,
            3600,
        )
        .await
        .unwrap();
        crate::proxy_admin::create_pool(
            &db,
            "未到",
            "extract",
            "http",
            "https://example.com/b",
            60,
            3600,
        )
        .await
        .unwrap();
        crate::proxy_admin::create_pool(
            &db,
            "关闭",
            "extract",
            "http",
            "https://example.com/c",
            60,
            0,
        )
        .await
        .unwrap();
        let due_id = due["id"].as_i64().unwrap();
        let later: i64 = sqlx::query_scalar("SELECT id FROM proxy_pools WHERE name = '未到'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE proxy_pools SET last_extract_at = 1000 WHERE id = ?")
            .bind(later)
            .execute(db.pool())
            .await
            .unwrap();
        let mut calls = 0;
        let fetched = crate::proxy_admin::refresh_due(&db, 2000, |_| {
            calls += 1;
            Ok(r#"{"data":[{"ip":"9.9.9.9","port":8000}]}"#.to_string())
        })
        .await
        .unwrap();
        assert_eq!(fetched, 1);
        assert_eq!(calls, 1);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proxies WHERE pool_id = ?")
            .bind(due_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
        let again = crate::proxy_admin::refresh_due(&db, 2000, |_| {
            calls += 1;
            Ok("".into())
        })
        .await
        .unwrap();
        assert_eq!(again, 0);
        assert_eq!(calls, 1);
        let failed = crate::proxy_admin::refresh_due(&db, 2000 + 3600, |_| Err("超时".into()))
            .await
            .unwrap();
        assert_eq!(failed, 0);
        let error: String = sqlx::query_scalar("SELECT last_error FROM proxy_pools WHERE id = ?")
            .bind(due_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(error, "超时");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn cookie_keepalive_respects_interval_and_alerts_once() {
        let path = std::env::temp_dir().join(format!(
            "vpush-cookie-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let mut calls = 0;
        cookie_keepalive_with(&db, 1_000, |_, _, _| {
            calls += 1;
            async { Keep::Dead("过期".into()) }
        })
        .await
        .unwrap();
        assert_eq!(calls, 0);
        db.set_setting("config_cookie_keepalive_interval_seconds", "100")
            .await
            .unwrap();
        db.set_setting("xueqiu_cookie", "xq=1").await.unwrap();
        db.add_kol("xueqiu", "段永平", "111", None, false, false, false)
            .await
            .unwrap();
        db.set_setting("weibo_cookie", "SUB=1").await.unwrap();
        cookie_keepalive_with(&db, 2_000, |_, _, _| {
            calls += 1;
            async { Keep::Alive }
        })
        .await
        .unwrap();
        assert_eq!(calls, 1);
        db.add_kol("weibo", "乙", "222", None, false, false, false)
            .await
            .unwrap();
        cookie_keepalive_with(&db, 2_050, |_, _, _| {
            calls += 1;
            async { Keep::Dead("过期".into()) }
        })
        .await
        .unwrap();
        assert_eq!(calls, 1);
        cookie_keepalive_with(&db, 2_200, |platform, _, _| {
            calls += 1;
            let platform = platform.to_string();
            async move {
                if platform == "weibo" {
                    Keep::Renewed("SUB=new".into())
                } else {
                    Keep::Alive
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(db.setting("source_err_xueqiu").await.unwrap().unwrap(), "");
        assert_eq!(
            db.setting("weibo_cookie").await.unwrap().unwrap(),
            "SUB=new"
        );
        cookie_keepalive_with(&db, 2_300, |_, _, _| {
            calls += 1;
            async { Keep::Dead("过期".into()) }
        })
        .await
        .unwrap();
        cookie_keepalive_with(&db, 2_500, |_, _, _| {
            calls += 1;
            async { Keep::Dead("又过期".into()) }
        })
        .await
        .unwrap();
        let alerts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM admin_logs WHERE action = 'cookie_keepalive'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(alerts, 1);
        assert_eq!(
            db.setting("source_err_xueqiu").await.unwrap().unwrap(),
            "又过期"
        );
        cookie_keepalive_with(&db, 2_700, |_, _, _| {
            calls += 1;
            async { Keep::Transient }
        })
        .await
        .unwrap();
        assert_eq!(
            db.setting("source_err_xueqiu").await.unwrap().unwrap(),
            "又过期"
        );
        let _ = std::fs::remove_file(&path);
    }
}
