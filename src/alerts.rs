//! 管理员告警。频率和文案对齐原来的调度器，发送函数可注入。

use serde_json::{json, Value};
use sqlx::Row;

use crate::db::Db;

pub async fn notify_admins(db: &Db, message: &str) {
    let Ok(ids) = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE is_admin = 1 ORDER BY id").fetch_all(db.pool()).await else {
        return;
    };
    let message = redact(message);
    for id in ids {
        if let Err(err) = crate::push::send_user_text(db, id, &message).await {
            tracing::warn!(user = id, "管理员告警发送失败: {err}");
        }
    }
}

pub async fn backup_failure<F, Fut>(db: &Db, detail: &str, mut send: F) -> Result<bool, sqlx::Error>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if !enabled() {
        return Ok(false);
    }
    let today = beijing_date(db).await?;
    if db.setting("backup_alert_date").await?.as_deref() == Some(today.as_str()) {
        return Ok(false);
    }
    db.set_setting("backup_alert_date", &today).await?;
    let message = format!("⚠️ 定时备份失败：{}", clip(detail, 200));
    send(message.clone()).await;
    db.add_admin_log(0, "backup_alert", "", &message).await?;
    Ok(true)
}

pub async fn push_failure<F, Fut>(db: &Db, now: i64, detail: &str, mut send: F) -> Result<bool, sqlx::Error>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if !enabled() {
        return Ok(false);
    }
    let last = db.setting("push_alert_last_at").await?.and_then(|value| value.parse::<i64>().ok()).unwrap_or(0);
    if last > 0 && now.saturating_sub(last) < 3600 {
        return Ok(false);
    }
    db.set_setting("push_alert_last_at", &now.to_string()).await?;
    let message = format!("⚠️ 用户推送失败（每小时最多提醒一次）：{}", clip(detail, 200));
    send(message.clone()).await;
    db.add_admin_log(0, "push_alert", "", &message).await?;
    Ok(true)
}

pub enum Probe {
    Alive,
    Dead,
    Http(u16),
    Error(String),
}

pub async fn probe_xueqiu<F, Fut>(db: &Db, now: i64, mut probe: F) -> Result<(), sqlx::Error>
where
    F: FnMut(&str, &str) -> Fut,
    Fut: std::future::Future<Output = Probe>,
{
    let interval = number(db, "config_source_probe_interval_seconds", 600).await?;
    if interval <= 0 {
        return Ok(());
    }
    let last = number(db, "xueqiu_probe_last_at", 0).await?;
    if last > 0 && now.saturating_sub(last) < interval {
        return Ok(());
    }
    db.set_setting("xueqiu_probe_last_at", &now.to_string()).await?;
    let cookie = db.setting("xueqiu_cookie").await?.unwrap_or_default();
    if cookie.trim().is_empty() {
        return Ok(());
    }
    let uid: Option<String> = sqlx::query_scalar("SELECT external_id FROM kols WHERE platform = 'xueqiu' AND enabled = 1 ORDER BY id LIMIT 1")
        .fetch_optional(db.pool())
        .await?;
    let Some(uid) = uid.filter(|value| !value.trim().is_empty()) else { return Ok(()) };
    match probe(cookie.trim(), uid.trim()).await {
        Probe::Alive => {
            db.set_setting("source_ok_xueqiu", &now.to_string()).await?;
            db.set_setting("source_err_xueqiu", "").await?;
        }
        Probe::Dead => {
            db.set_setting("source_err_xueqiu", "接口异常（探测）").await?;
            let alerted = number(db, "xueqiu_probe_alert_at", 0).await?;
            if alerted == 0 || now.saturating_sub(alerted) >= 6 * 3600 {
                db.set_setting("xueqiu_probe_alert_at", &now.to_string()).await?;
                let message = "⚠️ 雪球探测异常：抓取接口返回异常，cookie 可能失效。请到后台「数据源 → Cookie 管理」粘贴新的雪球 Cookie。";
                notify_admins(db, message).await;
                db.add_admin_log(0, "xueqiu_probe", "", message).await?;
            }
        }
        Probe::Http(status) => {
            db.set_setting("source_err_xueqiu", &format!("探测 HTTP {status}")).await?;
        }
        Probe::Error(err) => {
            db.set_setting("source_err_xueqiu", &clip(&err, 300)).await?;
        }
    }
    Ok(())
}

const PLATFORMS: &[(&str, &str)] = &[
    ("combination", "雪球组合"),
    ("ima", "ima"),
    ("truth", "Truth Social"),
    ("twitter", "X"),
    ("weibo", "微博"),
    ("xueqiu", "雪球"),
    ("zsxq", "知识星球"),
];

pub async fn source_health<F, Fut>(db: &Db, now: i64, mut send: F) -> Result<bool, sqlx::Error>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if !enabled() {
        return Ok(false);
    }
    let checked = number(db, "source_health_check_at", 0).await?;
    if checked > 0 && now.saturating_sub(checked) < 600 {
        return Ok(false);
    }
    db.set_setting("source_health_check_at", &now.to_string()).await?;
    let alerted = number(db, "source_health_alert_at", 0).await?;
    if alerted > 0 && now.saturating_sub(alerted) < 6 * 3600 {
        return Ok(false);
    }
    let rows = sqlx::query(
        "SELECT k.platform, k.last_fetch_at, k.last_fetch_error FROM kols k \
         WHERE k.enabled = 1 AND EXISTS (SELECT 1 FROM subscriptions s WHERE s.kol_id = k.id)",
    )
    .fetch_all(db.pool())
    .await?;
    let mut issues = Vec::new();
    for (platform, label) in PLATFORMS {
        let tracked: Vec<_> = rows.iter().filter(|row| row.get::<String, _>("platform") == *platform).collect();
        if tracked.is_empty() {
            continue;
        }
        let (ok, fail) = event_counts(db, platform).await?;
        let total = ok + fail;
        if total >= 10 {
            let rate = (ok as f64 * 100.0 / total as f64).round() as i64;
            if rate < 70 {
                issues.push(format!("{label}：24h 成功率 {rate}%（成功 {ok}/失败 {fail}）"));
            }
        }
        let mut success_at = 0i64;
        let mut fetched_at = 0i64;
        for row in &tracked {
            let at = row.get::<String, _>("last_fetch_at").parse::<i64>().unwrap_or(0);
            if at == 0 {
                continue;
            }
            fetched_at = fetched_at.max(at);
            if row.get::<String, _>("last_fetch_error").is_empty() {
                success_at = success_at.max(at);
            }
        }
        let anchor = if success_at > 0 { success_at } else { fetched_at };
        if anchor > 0 && (now - anchor) / 3600 >= 6 {
            issues.push(format!("{label}：已 {} 小时无成功抓取", (now - anchor) / 3600));
        }
    }
    if issues.is_empty() {
        return Ok(false);
    }
    db.set_setting("source_health_alert_at", &now.to_string()).await?;
    let message = format!("⚠️ 数据源健康告警\n{}", issues.iter().map(|item| format!("· {item}")).collect::<Vec<_>>().join("\n"));
    send(message.clone()).await;
    db.add_admin_log(0, "source_health", "", &message).await?;
    Ok(true)
}

async fn event_counts(db: &Db, platform: &str) -> Result<(i64, i64), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT status, SUM(CASE WHEN ok_count > 0 THEN ok_count ELSE 1 END) AS ok, \
         SUM(CASE WHEN fail_count > 0 THEN fail_count ELSE 1 END) AS fail \
         FROM source_events WHERE platform = ? AND created_at >= datetime('now', '-24 hours') GROUP BY status",
    )
    .bind(platform)
    .fetch_all(db.pool())
    .await?;
    let mut ok = 0;
    let mut fail = 0;
    for row in rows {
        match row.get::<String, _>("status").as_str() {
            "ok" => ok = row.get("ok"),
            "fail" => fail = row.get("fail"),
            _ => {}
        }
    }
    Ok((ok, fail))
}

pub fn evaluate_cicc(status: &Value, settings: &Value, now: i64) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let pct = status.pointer("/storage/disk/pct").and_then(Value::as_f64).unwrap_or(0.0);
    let crit = settings.get("disk_crit").and_then(Value::as_i64).unwrap_or(90);
    let warn = settings.get("disk_warn").and_then(Value::as_i64).unwrap_or(80);
    if pct >= crit as f64 {
        out.push(("disk_crit".into(), format!("🔴 存储机磁盘使用率 {pct}%（≥{crit}%），请尽快清理归档。")));
    } else if pct >= warn as f64 {
        out.push(("disk_warn".into(), format!("🟡 存储机磁盘使用率 {pct}%（≥{warn}%）。")));
    }
    let ts = status.get("ts").and_then(Value::as_i64).unwrap_or(0);
    let stale_min = settings.get("stale_minutes").and_then(Value::as_i64).unwrap_or(30).max(1);
    if ts > 0 && now.saturating_sub(ts) > stale_min * 60 {
        out.push(("stale".into(), format!("⚠️ 存储机状态已超过 {stale_min} 分钟未刷新，采集/状态服务可能异常。")));
    }
    out
}

pub async fn check_cicc<F, Fut>(db: &Db, now: i64, status: &Value, mut send: F) -> Result<usize, sqlx::Error>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut state = db.setting("cicc_alert_state").await?.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()).unwrap_or_else(|| json!({}));
    let last = state.get("last_check").and_then(Value::as_i64).unwrap_or(0);
    if last > 0 && now.saturating_sub(last) < 300 {
        return Ok(0);
    }
    state["last_check"] = json!(now);
    let settings = cicc_settings(db).await?;
    let mut pending = Vec::new();
    let notify = settings.get("notify_enabled").and_then(Value::as_bool).unwrap_or(true);
    if notify && status.as_object().is_some_and(|item| !item.is_empty()) && status.get("available") != Some(&json!(false)) {
        pending.extend(evaluate_cicc(status, &settings, now));
        if let Some(alert) = paused_alert(status, &state) {
            if let Some(ts) = status.pointer("/paused/ts").and_then(Value::as_i64) {
                state["paused_notified_ts"] = json!(ts);
            }
            pending.push(alert);
        }
        let summary_ts = status.pointer("/storage/last_incr_summary/ts").and_then(Value::as_i64).unwrap_or(0);
        if summary_ts > state.get("incr_notified_ts").and_then(Value::as_i64).unwrap_or(0) {
            let added = status.pointer("/storage/last_incr_summary/added").and_then(Value::as_i64).unwrap_or(0);
            let failed = status.pointer("/storage/last_incr_summary/failed").and_then(Value::as_i64).unwrap_or(0);
            pending.push(("__incr__".into(), format!("📥 中金增量完成：新增 {added} 篇，失败 {failed} 篇。")));
            state["incr_notified_ts"] = json!(summary_ts);
        }
    }
    let mut sent = 0;
    for (key, message) in pending {
        if key != "__incr__" && !cooled(&state, &key, now) {
            continue;
        }
        send(message.clone()).await;
        sent += 1;
        if key != "__incr__" {
            let alerts = state.as_object_mut().and_then(|item| item.entry("alerts").or_insert_with(|| json!({})).as_object_mut());
            if let Some(alerts) = alerts {
                alerts.insert(key, json!(now));
            }
        }
    }
    db.set_setting("cicc_alert_state", &state.to_string()).await?;
    Ok(sent)
}

fn paused_alert(status: &Value, state: &Value) -> Option<(String, String)> {
    let paused = status.get("paused")?;
    let ts = paused.get("ts").and_then(Value::as_i64).unwrap_or(0);
    if ts <= state.get("paused_notified_ts").and_then(Value::as_i64).unwrap_or(0) {
        return None;
    }
    let reason = paused.get("reason").and_then(Value::as_str).unwrap_or("");
    let message = match reason {
        "quota" => "🔴 中金采集暂停：本月研报配额已满，等月初重置（每日增量会自动重试）。".into(),
        "auth" => "🔴 中金采集暂停：登录态失效，请在存储机更新 Cookie 文件。".into(),
        _ => format!("🔴 中金采集暂停：{}。", paused.get("detail").and_then(Value::as_str).filter(|item| !item.is_empty()).unwrap_or(if reason.is_empty() { "未知原因" } else { reason })),
    };
    Some(("paused".into(), message))
}

fn cooled(state: &Value, key: &str, now: i64) -> bool {
    let last = state.pointer(&format!("/alerts/{key}")).and_then(Value::as_i64).unwrap_or(0);
    last == 0 || now.saturating_sub(last) >= 86_400
}

async fn cicc_settings(db: &Db) -> Result<Value, sqlx::Error> {
    let mut settings = json!({"disk_warn": 80, "disk_crit": 90, "stale_minutes": 30, "notify_enabled": true});
    if let Some(raw) = db.setting("cicc_alert_settings").await? {
        if let Ok(Value::Object(extra)) = serde_json::from_str::<Value>(&raw) {
            if let Some(base) = settings.as_object_mut() {
                for (key, value) in extra {
                    base.insert(key, value);
                }
            }
        }
    }
    Ok(settings)
}

pub(crate) fn alerts_enabled() -> bool {
    enabled()
}

fn enabled() -> bool {
    !matches!(std::env::var("ALERTS_ENABLED").ok().as_deref(), Some("0" | "false" | "no" | "off"))
}

pub(crate) fn platform_wide(detail: &str) -> bool {
    ["cookie", "WAF", "反爬", "登录", "限流"].iter().any(|token| detail.contains(token)) || detail.to_lowercase().contains("login") || detail.contains("HTTP 429")
}

pub(crate) fn terminal_kol(detail: &str) -> bool {
    ["未找到用户", "用户不存在或已停用", "UserUnavailable", "无法识别 X 用户名"].iter().any(|token| detail.contains(token))
}

fn label(platform: &str) -> &str {
    PLATFORMS.iter().find(|(key, _)| *key == platform).map(|(_, name)| *name).unwrap_or(platform)
}

pub(crate) fn kol_failure_message(platform: &str, name: &str, detail: &str, count: i64) -> String {
    format!("⚠️ 数据源告警：{}「{name}」连续失败 {count} 次。\n错误：{}", label(platform), clip(detail, 200))
}

pub(crate) fn kol_disabled_message(platform: &str, name: &str, detail: &str, count: i64) -> String {
    format!("⏸️ 已自动停用：{}「{name}」连续失败 {count} 次，已暂停抓取。\n错误：{}\n可在大V管理里重新启用。", label(platform), clip(detail, 200))
}

pub(crate) fn kol_recovered_message(platform: &str, name: &str) -> String {
    format!("✅ 数据源已恢复：{}「{name}」重新抓取成功。", label(platform))
}

pub(crate) fn weibo_login_message(detail: &str) -> String {
    format!("⚠️ 微博 cookie 自动登录失败，请检查 weibo.username/password 或手动更新 cookie。详情：{}", clip(detail, 200))
}

pub(crate) fn xueqiu_cookie_message(detail: &str) -> String {
    format!("⚠️ 雪球 cookie 失效，请到后台「数据源 → Cookie 管理」粘贴新 Cookie。详情：{}", clip(detail, 200))
}

async fn beijing_date(db: &Db) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT date('now', '+8 hours')").fetch_one(db.pool()).await
}

async fn number(db: &Db, key: &str, default: i64) -> Result<i64, sqlx::Error> {
    Ok(db.setting(key).await?.and_then(|value| value.parse().ok()).unwrap_or(default))
}

fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn redact(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("key=") {
        out.push_str(&rest[..start + 4]);
        rest = &rest[start + 4..];
        let end = rest.find(|item: char| item == '&' || item.is_whitespace()).unwrap_or(rest.len());
        out.push_str("***");
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("vpush-alerts-{}-{}-{}.db", std::process::id(), n, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
    }

    #[tokio::test]
    async fn backup_and_push_alerts_respect_their_windows() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        let mut sent = Vec::new();
        assert!(backup_failure(&db, "WebDAV 连不上", |message| {
            sent.push(message);
            async {}
        })
        .await
        .unwrap());
        assert!(!backup_failure(&db, "再次", |_| async {}).await.unwrap());
        assert!(sent[0].contains("定时备份失败"));
        assert!(sent[0].contains("WebDAV 连不上"));
        assert!(push_failure(&db, 1_000, "企业微信超时", |message| {
            sent.push(message);
            async {}
        })
        .await
        .unwrap());
        assert!(!push_failure(&db, 2_000, "不应再发", |_| async {}).await.unwrap());
        assert!(push_failure(&db, 5_000, "一小时后", |message| {
            sent.push(message);
            async {}
        })
        .await
        .unwrap());
        assert_eq!(sent.len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn xueqiu_probe_alerts_only_when_the_session_is_dead() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        db.set_setting("config_source_probe_interval_seconds", "10").await.unwrap();
        db.set_setting("xueqiu_cookie", "xq=1").await.unwrap();
        db.add_kol("xueqiu", "段永平", "111", None, false, false, false).await.unwrap();
        probe_xueqiu(&db, 100, |_, _| async { Probe::Http(500) }).await.unwrap();
        assert_eq!(db.setting("source_err_xueqiu").await.unwrap().as_deref(), Some("探测 HTTP 500"));
        assert!(db.setting("xueqiu_probe_alert_at").await.unwrap().is_none());
        probe_xueqiu(&db, 120, |_, uid| {
            assert_eq!(uid, "111");
            async { Probe::Dead }
        })
        .await
        .unwrap();
        assert_eq!(db.setting("xueqiu_probe_alert_at").await.unwrap().as_deref(), Some("120"));
        probe_xueqiu(&db, 130, |_, _| async { Probe::Dead }).await.unwrap();
        assert_eq!(db.setting("xueqiu_probe_alert_at").await.unwrap().as_deref(), Some("120"));
        probe_xueqiu(&db, 120 + 6 * 3600, |_, _| async { Probe::Alive }).await.unwrap();
        assert_eq!(db.setting("source_err_xueqiu").await.unwrap().as_deref(), Some(""));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn source_health_alerts_on_low_rate_and_silence() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('reader', 'x')").execute(db.pool()).await.unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'reader'").fetch_one(db.pool()).await.unwrap();
        let watched = db.add_kol("xueqiu", "段永平", "111", None, false, false, false).await.unwrap();
        let quiet = db.add_kol("weibo", "乙", "222", None, false, false, false).await.unwrap();
        let ignored = db.add_kol("zsxq", "丙", "333", None, false, false, false).await.unwrap();
        for kol in [watched, quiet] {
            sqlx::query("INSERT INTO subscriptions (user_id, kol_id, type) VALUES (?, ?, 'post')").bind(user).bind(kol).execute(db.pool()).await.unwrap();
        }
        let now = 20_000_000i64;
        sqlx::query("UPDATE kols SET last_fetch_at = ?, last_fetch_error = '' WHERE id = ?").bind((now - 60).to_string()).bind(watched).execute(db.pool()).await.unwrap();
        sqlx::query("UPDATE kols SET last_fetch_at = ?, last_fetch_error = '超时' WHERE id = ?").bind((now - 7 * 3600).to_string()).bind(quiet).execute(db.pool()).await.unwrap();
        sqlx::query("UPDATE kols SET last_fetch_at = ? WHERE id = ?").bind((now - 9 * 3600).to_string()).bind(ignored).execute(db.pool()).await.unwrap();
        for _ in 0..8 {
            sqlx::query("INSERT INTO source_events (platform, status, fail_count) VALUES ('xueqiu', 'fail', 1)").execute(db.pool()).await.unwrap();
        }
        sqlx::query("INSERT INTO source_events (platform, status, ok_count) VALUES ('xueqiu', 'ok', 2)").execute(db.pool()).await.unwrap();
        let mut sent = Vec::new();
        assert!(source_health(&db, now, |message| {
            sent.push(message);
            async {}
        })
        .await
        .unwrap());
        assert!(sent[0].contains("雪球：24h 成功率 20%（成功 2/失败 8）"));
        assert!(sent[0].contains("微博：已 7 小时无成功抓取"));
        assert!(!sent[0].contains("知识星球"));
        assert!(!source_health(&db, now + 700, |_| async {}).await.unwrap());
        assert!(source_health(&db, now + 6 * 3600 + 700, |message| {
            sent.push(message);
            async {}
        })
        .await
        .unwrap());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn kol_failures_alert_disable_and_recover() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        let gone = db.add_kol("twitter", "消失", "404", None, false, false, false).await.unwrap();
        let alive = db.add_kol("xueqiu", "段永平", "111", None, false, false, false).await.unwrap();
        for _ in 0..4 {
            db.note_kol_fetch(gone, Some("未找到用户")).await.unwrap();
        }
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT enabled FROM kols WHERE id = ?").bind(gone).fetch_one(db.pool()).await.unwrap(), 1);
        db.note_kol_fetch(gone, Some("未找到用户")).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT enabled FROM kols WHERE id = ?").bind(gone).fetch_one(db.pool()).await.unwrap(), 0);
        let disabled: String = sqlx::query_scalar("SELECT detail FROM admin_logs WHERE action = 'kol_disabled'").fetch_one(db.pool()).await.unwrap();
        assert!(disabled.contains("已自动停用"));
        assert!(disabled.contains("消失"));
        for _ in 0..5 {
            db.note_kol_fetch(alive, Some("cookie 失效")).await.unwrap();
        }
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT enabled FROM kols WHERE id = ?").bind(alive).fetch_one(db.pool()).await.unwrap(), 1);
        let failure: String = sqlx::query_scalar("SELECT detail FROM admin_logs WHERE action = 'kol_failure'").fetch_one(db.pool()).await.unwrap();
        assert!(failure.contains("连续失败 3 次"));
        db.note_kol_fetch(alive, None).await.unwrap();
        let recovered: String = sqlx::query_scalar("SELECT detail FROM admin_logs WHERE action = 'kol_recovered'").fetch_one(db.pool()).await.unwrap();
        assert!(recovered.contains("重新抓取成功"));
        sqlx::query("INSERT INTO source_events (platform, status, created_at) VALUES ('xueqiu', 'ok', '2000-01-01')").execute(db.pool()).await.unwrap();
        db.prune_retention_if_due().await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM source_events WHERE created_at = '2000-01-01'").fetch_one(db.pool()).await.unwrap(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn cicc_alerts_cooldown_and_increment_notice() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        let status = json!({
            "ts": 1_000,
            "storage": {"disk": {"pct": 91}, "last_incr_summary": {"ts": 50, "added": 3, "failed": 1}},
            "paused": {"ts": 40, "reason": "auth"}
        });
        let mut sent = Vec::new();
        let count = check_cicc(&db, 1_000 + 31 * 60, &status, |message| {
            sent.push(message);
            async {}
        })
        .await
        .unwrap();
        assert_eq!(count, 4);
        assert!(sent.iter().any(|item| item.contains("磁盘使用率")));
        assert!(sent.iter().any(|item| item.contains("未刷新")));
        assert!(sent.iter().any(|item| item.contains("登录态失效")));
        assert!(sent.iter().any(|item| item.contains("新增 3 篇")));
        sent.clear();
        let again = check_cicc(&db, 1_000 + 40 * 60, &status, |message| {
            sent.push(message);
            async {}
        })
        .await
        .unwrap();
        assert_eq!(again, 0);
        assert!(sent.is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
